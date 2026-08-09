//! The Material 3 Expressive **connected button group**: a horizontal row of
//! connected, single-select (segmented) buttons.
//!
//! [`button_group`] takes the member labels, the currently-`selected` index,
//! and an `on_select` callback. It is a **controlled component** (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics): a tap on a member fires
//! `on_select(index)` and leaves `selected` untouched — the app feeds the new
//! selection back on the next rebuild, exactly like [`crate::Switch`]/
//! [`crate::checkbox`] report a *requested* value rather than self-mutating.
//!
//! # Connected geometry (inner vs. outer corners)
//!
//! The members abut with no gap, so the group reads as one connected shape. The
//! group's *outer* corners (the leftmost member's left corners, the rightmost
//! member's right corners) use the group shape token ([`ShapeScale::large`](frust_theme::ShapeScale::large),
//! 16dp fallback); every *inner*, adjacent corner uses a smaller radius
//! ([`ShapeScale::extra_small`](frust_theme::ShapeScale::extra_small), 4dp fallback). A single-member group is all
//! outer corners. This is [`member_radii`], unit-tested directly. Because
//! `PaintScene` has no per-corner rounded-rect primitive, each member's
//! background is built as a `kurbo` [`RoundedRect`] with per-corner
//! [`RoundedRectRadii`] and filled via [`PaintScene::fill_path`] — the same
//! technique [`super::sheet`]/[`super::card`] use.
//!
//! # Pressed-member shape emphasis (the `shape_morph` consumer)
//!
//! Pressing a member paints a translucent **morphing shape** over it: a rounded
//! square that springs rounder as the press-spring advances, drawn via
//! [`super::shape_morph::morph_path`] (this module is that primitive's second
//! consumer, after [`super::loading_indicator`]). The spring is
//! [`PRESS_SPRING`], the exact `MotionScheme::m3_expressive().fast_spatial`
//! preset (stiffness 1400, ζ 0.9) — a snappy, slightly-overshooting pop. It is
//! a named constant rather than a paint-time theme read because a press starts
//! in the event pass, and event-pass code never reads a theme (see
//! `docs/CODE_STANDARDS.md`'s Theming conventions); `press_spring_matches_motion_scheme`
//! is the tripwire that keeps it equal to the token.
//!
//! # Scope
//!
//! Single-select (segmented) only; multi-select is out of scope for v1 (an
//! optional M3 variant this module does not implement). The member labels use the
//! [`ThemeTextColor::OnSurface`] role uniformly — selection is conveyed by the
//! member's container fill (`secondary_container`), not a distinct label color,
//! to avoid adding an `on_secondary_container` text role in `text.rs` (a file
//! outside this module's scope). Semantics expose a [`Role::RadioGroup`] of
//! per-member [`Role::RadioButton`] nodes; their bounds are the whole group's
//! (v1 — `SemanticsCtx` exposes no public per-sub-rect descent), so linear
//! (by-label) screen-reader navigation is correct while spatial is approximate.

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

use super::shape_morph::{RoundedPolygon, morph_path};
use super::state_layer::PRESSED_OPACITY;
use crate::authoring::ThemeTextColor;
use crate::text;

/// Horizontal padding around each member's label, in logical px.
const PAD_X: f64 = 16.0;
/// Vertical padding around each member's label, in logical px.
const PAD_Y: f64 = 10.0;
/// Tolerance for flattening `kurbo` rounded-rect paths (matches
/// [`super::sheet`]'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback outer corner radius (a theme resolves [`ShapeScale::large`](frust_theme::ShapeScale::large)).
const OUTER_RADIUS: f64 = 16.0;
/// Unthemed-fallback inner (adjacent) corner radius (a theme resolves
/// [`ShapeScale::extra_small`](frust_theme::ShapeScale::extra_small)).
const INNER_RADIUS: f64 = 4.0;

/// Unthemed-fallback selected-member container fill (a theme resolves
/// `colors.secondary_container`).
const SELECTED_FILL: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback unselected-member container fill (a theme resolves
/// `colors.surface`).
const UNSELECTED_FILL: Color = Color::from_rgb8(0xFE, 0xF7, 0xFF);
/// Unthemed-fallback member outline (a theme resolves `colors.outline_variant`).
const OUTLINE: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);
/// Unthemed-fallback member content color, tinting the pressed emphasis (a
/// theme resolves `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// Member-outline stroke width, in logical px.
const OUTLINE_WIDTH: f64 = 1.0;

/// The press-emphasis morph spring: the exact
/// `MotionScheme::m3_expressive().fast_spatial` preset (stiffness 1400, ζ 0.9,
/// mass 1). See the [module docs](self) for why it is a constant rather than a
/// theme read.
const PRESS_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 1400.0,
    damping_ratio: 0.9,
};

/// The press morph's start shape: a lightly-rounded axis-aligned square
/// (rotation π/4 orients a 4-gon's flat edges to the axes).
const EMPHASIS_FROM: RoundedPolygon = RoundedPolygon::new(4, 0.30, std::f64::consts::FRAC_PI_4);
/// The press morph's end shape: a much-rounder square (the "shape emphasis" the
/// press springs toward).
const EMPHASIS_TO: RoundedPolygon = RoundedPolygon::new(4, 0.80, std::f64::consts::FRAC_PI_4);

/// The circumradius of the pressed-member emphasis shape, as a fraction of the
/// member's shorter side — a hair inside so it sits within the member.
const EMPHASIS_RADIUS_FRAC: f64 = 0.46;

/// Nominal period seeding the press [`AnimationController`]'s clock; the motion
/// is spring-driven ([`PRESS_SPRING`]) via `fling`, so this duration only backs
/// the controller's construction and is not itself a timing.
const PRESS_ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (value-units/sec) handed to the press-in / press-out
/// [`AnimationController::fling`] — a modest kick so the spring reads snappy.
const FLING_VELOCITY: f64 = 4.0;

/// A view-held, typed selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// Build one member's label view, themed [`ThemeTextColor::OnSurface`]. Shared
/// by build/rebuild/teardown so the role stays consistent.
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    frust_core::any::<State, _>(text::text(label).themed_role(ThemeTextColor::OnSurface))
}

/// The per-corner radii for member `index` of a `count`-member connected group:
/// outer corners (group ends) get `outer`, inner (adjacent) corners get `inner`.
///
/// - `count == 1`: all four corners `outer` (a standalone rounded button).
/// - leftmost (`index == 0`): left corners `outer`, right corners `inner`.
/// - rightmost (`index == count - 1`): left `inner`, right `outer`.
/// - middle: all four `inner`.
pub(crate) fn member_radii(index: usize, count: usize, outer: f64, inner: f64) -> RoundedRectRadii {
    let is_first = index == 0;
    let is_last = index + 1 == count;
    // top_left, top_right, bottom_right, bottom_left.
    let left = if is_first { outer } else { inner };
    let right = if is_last { outer } else { inner };
    RoundedRectRadii::new(left, right, right, left)
}

/// A declarative connected button group. See the [module docs](self).
pub struct ButtonGroupView<State: 'static> {
    labels: Vec<String>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Create a single-select connected button group with the given member
/// `labels`, the currently-`selected` index, and an `on_select` callback fired
/// (with the tapped member's index) on release inside a member.
///
/// A `selected` index outside `0..labels.len()` simply highlights no member
/// (there is no panic); feed a valid index to show a selection.
pub fn button_group<State: 'static, F: Fn(&mut State, usize) + 'static>(
    labels: impl IntoIterator<Item = impl Into<String>>,
    selected: usize,
    on_select: F,
) -> ButtonGroupView<State> {
    ButtonGroupView {
        labels: labels.into_iter().map(Into::into).collect(),
        selected,
        on_select: Rc::new(on_select),
    }
}

/// PascalCase alias for [`button_group`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn ButtonGroup<State: 'static, F: Fn(&mut State, usize) + 'static>(
    labels: impl IntoIterator<Item = impl Into<String>>,
    selected: usize,
    on_select: F,
) -> ButtonGroupView<State> {
    button_group(labels, selected, on_select)
}

/// The retained widget for a [`ButtonGroupView`].
pub struct ButtonGroupWidget {
    /// One label [`TextWidget`](crate::TextWidget) pod per member.
    members: Vec<ChildPod>,
    /// Member label strings, retained for the semantics nodes.
    labels: Vec<String>,
    /// The app-confirmed selected index (controlled — never self-mutated).
    selected: usize,
    /// Per-member background rects in the widget's local space, computed in
    /// [`Widget::layout`] and used for paint + hit-testing.
    member_rects: Vec<Rect>,
    /// Whether a press is currently armed (a `Down` landed on a member).
    armed: bool,
    /// The member whose press emphasis is animating (persists through the
    /// release spring until it settles back to rest).
    active_member: Option<usize>,
    /// Whether the armed pointer is currently inside [`Self::active_member`].
    pressed_inside: bool,
    /// The spring-driven press-emphasis morph parameter (0 rest → 1 pressed).
    press_anim: frust_core::AnimationController,
    on_select: crate::authoring::ErasedArgCallback<usize>,
}

/// The `(selected_fill, unselected_fill, outline, content)` colors. Themed:
/// `secondary_container`/`surface`/`outline_variant`/`on_surface`. Unthemed: the
/// module fallback constants exactly.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.secondary_container,
                s.surface,
                s.outline_variant,
                s.on_surface,
            )
        }
        None => (SELECTED_FILL, UNSELECTED_FILL, OUTLINE, ON_SURFACE),
    }
}

/// The `(outer, inner)` corner radii. Themed: `shape.large`/`shape.extra_small`.
/// Unthemed: [`OUTER_RADIUS`]/[`INNER_RADIUS`].
fn resolve_radii(theme: Option<&Theme>) -> (f64, f64) {
    match theme {
        Some(theme) => (theme.shape.large, theme.shape.extra_small),
        None => (OUTER_RADIUS, INNER_RADIUS),
    }
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

impl ButtonGroupWidget {
    /// Which member (if any) contains local point `pos`.
    fn member_at(&self, pos: Point) -> Option<usize> {
        self.member_rects.iter().position(|r| r.contains(pos))
    }

    /// Start the press-in emphasis morph.
    fn press_in(&mut self) {
        self.press_anim.fling(FLING_VELOCITY, PRESS_SPRING);
    }

    /// Start the press-out (release) morph back toward rest.
    fn press_out(&mut self) {
        self.press_anim.fling(-FLING_VELOCITY, PRESS_SPRING);
    }
}

impl<State: 'static> View<State> for ButtonGroupView<State> {
    type Element = ButtonGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ButtonGroupWidget {
        let members = self
            .labels
            .iter()
            .map(|label| crate::authoring::build_child(&label_view::<State>(label.clone()), ctx))
            .collect();
        ButtonGroupWidget {
            members,
            labels: self.labels.clone(),
            selected: self.selected,
            member_rects: Vec::new(),
            armed: false,
            active_member: None,
            pressed_inside: false,
            press_anim: frust_core::AnimationController::new(PRESS_ANIM_PERIOD),
            on_select: crate::authoring::erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = crate::authoring::erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;

        if prev.selected != self.selected {
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }

        // Reconcile the member label list. A count change is structural
        // (rebuild via the shared multi-child reconciler); a same-count change
        // rebuilds each label in place.
        if prev.labels.len() != self.labels.len() {
            let prev_views: Vec<AnyView<State>> = prev
                .labels
                .iter()
                .map(|l| label_view::<State>(l.clone()))
                .collect();
            let next_views: Vec<AnyView<State>> = self
                .labels
                .iter()
                .map(|l| label_view::<State>(l.clone()))
                .collect();
            flags |= crate::authoring::rebuild_children(
                &prev_views,
                &next_views,
                &mut element.members,
                ctx,
                |v| v,
                |_| None,
            );
            element.labels = self.labels.clone();
            // A structural change invalidates the recorded press.
            element.armed = false;
            element.active_member = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (i, (prev_label, next_label)) in
                prev.labels.iter().zip(self.labels.iter()).enumerate()
            {
                if prev_label != next_label {
                    let prev_view = label_view::<State>(prev_label.clone());
                    let next_view = label_view::<State>(next_label.clone());
                    flags |= crate::authoring::rebuild_child(
                        &prev_view,
                        &next_view,
                        &mut element.members[i],
                        ctx,
                    );
                }
            }
            element.labels = self.labels.clone();
        }

        flags
    }

    fn teardown(&self, element: &mut ButtonGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (label, pod) in self.labels.iter().zip(element.members.iter_mut()) {
            crate::authoring::teardown_child(&label_view::<State>(label.clone()), pod, ctx);
        }
    }
}

impl Widget for ButtonGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Lay out each label, then size each member to its label + padding. All
        // members share the tallest member height; each member keeps its own
        // width. Members abut left-to-right (connected group).
        let mut label_sizes = Vec::with_capacity(self.members.len());
        let mut height = 0.0_f64;
        for pod in self.members.iter_mut() {
            let ls = pod.layout_child(ctx, &BoxConstraints::loose(bc.max()));
            height = height.max(ls.height + PAD_Y * 2.0);
            label_sizes.push(ls);
        }

        self.member_rects.clear();
        let mut x = 0.0_f64;
        for (pod, ls) in self.members.iter_mut().zip(label_sizes.iter()) {
            let member_w = ls.width + PAD_X * 2.0;
            let rect = Rect::new(x, 0.0, x + member_w, height);
            pod.set_origin(Point::new(x + PAD_X, (height - ls.height) / 2.0));
            self.member_rects.push(rect);
            x += member_w;
        }

        bc.constrain(Size::new(x, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (selected_fill, unselected_fill, outline, content) = resolve_colors(theme);
        let (outer, inner) = resolve_radii(theme);
        let origin = ctx.origin();
        let count = self.member_rects.len();

        // Advance the press-emphasis spring; when it has settled at rest and no
        // press is armed, forget the active member.
        let animating = self.press_anim.advance(ctx.frame_time());
        if !animating && !self.armed && self.press_anim.value().abs() < 1e-3 {
            self.active_member = None;
        }

        for (i, rect) in self.member_rects.iter().enumerate() {
            let radii = member_radii(i, count, outer, inner);
            let path: BezPath = RoundedRect::from_rect(*rect, radii).to_path(PATH_TOLERANCE);
            let fill = if i == self.selected {
                selected_fill
            } else {
                unselected_fill
            };
            scene.fill_path(origin, &path, &Brush::Solid(fill));
            scene.stroke_path(origin, &path, OUTLINE_WIDTH, &Brush::Solid(outline));
        }

        // Pressed-member shape emphasis: a translucent morphing rounded square,
        // springing rounder as the press advances (the `shape_morph` consumer).
        if let Some(i) = self.active_member
            && let Some(rect) = self.member_rects.get(i)
        {
            let t = self.press_anim.value();
            let center = Point::new(rect.center().x, rect.center().y);
            let radius = rect.height().min(rect.width()) * EMPHASIS_RADIUS_FRAC;
            let path = morph_path(&EMPHASIS_FROM, &EMPHASIS_TO, t, center, radius);
            let alpha = (PRESSED_OPACITY as f64 * t.clamp(0.0, 1.0)) as f32;
            scene.fill_path(origin, &path, &Brush::Solid(with_alpha(content, alpha)));
        }

        for pod in self.members.iter_mut() {
            pod.paint_child(ctx, scene);
        }

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
                let Some(i) = self.member_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed = true;
                self.active_member = Some(i);
                self.pressed_inside = true;
                self.press_in();
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                let inside = self
                    .active_member
                    .and_then(|i| self.member_rects.get(i))
                    .is_some_and(|r| r.contains(p.position));
                if inside != self.pressed_inside {
                    self.pressed_inside = inside;
                    if inside {
                        self.press_in();
                    } else {
                        self.press_out();
                    }
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                if self.pressed_inside
                    && let Some(i) = self.active_member
                {
                    (self.on_select)(ctx, i);
                }
                self.armed = false;
                self.pressed_inside = false;
                self.press_out();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                self.armed = false;
                self.pressed_inside = false;
                self.press_out();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A single-select segmented control reads as a RadioGroup of
        // RadioButtons. Bounds are the whole group's (v1 — see the module docs);
        // the per-member label + selection are exposed directly (the internal
        // label pods are not forwarded, mirroring `Button`).
        ctx.push_container(
            Role::RadioGroup,
            |_| {},
            |ctx| {
                for (i, label) in self.labels.iter().enumerate() {
                    ctx.push_node(Role::RadioButton, |node| {
                        node.set_label(label.as_str());
                        node.set_selected(i == self.selected);
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    crate::authoring::visit_children!(members);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, PointerButton, PointerEvent};
    use frust_theme::MotionScheme;
    use kurbo::PathEl;
    use std::any::Any;

    fn build_group(labels: &[&str], selected: usize) -> ButtonGroupWidget {
        let view = button_group::<u32, _>(
            labels.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            selected,
            |s: &mut u32, i| *s = i as u32,
        );
        let mut counter = 0u64;
        View::<u32>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    /// Give the widget known member rects without a text-context layout (each
    /// member 40 wide, 48 tall, abutting from x=0).
    fn with_rects(w: &mut ButtonGroupWidget, count: usize) {
        w.member_rects = (0..count)
            .map(|i| Rect::new(i as f64 * 40.0, 0.0, i as f64 * 40.0 + 40.0, 48.0))
            .collect();
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ButtonGroupWidget, state: &mut u32, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 48.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn press_spring_matches_motion_scheme() {
        let fast = MotionScheme::m3_expressive().fast_spatial;
        assert_eq!(PRESS_SPRING.stiffness, fast.stiffness);
        assert_eq!(PRESS_SPRING.damping_ratio, fast.damping_ratio);
        assert_eq!(PRESS_SPRING.mass, 1.0);
    }

    #[test]
    fn single_member_group_is_all_outer_corners() {
        let r = member_radii(0, 1, 16.0, 4.0);
        assert_eq!(r.top_left, 16.0);
        assert_eq!(r.top_right, 16.0);
        assert_eq!(r.bottom_right, 16.0);
        assert_eq!(r.bottom_left, 16.0);
    }

    #[test]
    fn end_members_get_outer_on_their_outer_side_only() {
        // Leftmost: left corners outer, right corners inner.
        let first = member_radii(0, 3, 16.0, 4.0);
        assert_eq!((first.top_left, first.bottom_left), (16.0, 16.0));
        assert_eq!((first.top_right, first.bottom_right), (4.0, 4.0));
        // Rightmost: mirror.
        let last = member_radii(2, 3, 16.0, 4.0);
        assert_eq!((last.top_left, last.bottom_left), (4.0, 4.0));
        assert_eq!((last.top_right, last.bottom_right), (16.0, 16.0));
    }

    #[test]
    fn middle_members_are_all_inner_corners() {
        let mid = member_radii(1, 3, 16.0, 4.0);
        assert_eq!(mid.top_left, 4.0);
        assert_eq!(mid.top_right, 4.0);
        assert_eq!(mid.bottom_right, 4.0);
        assert_eq!(mid.bottom_left, 4.0);
    }

    #[test]
    fn build_creates_one_pod_per_member() {
        let w = build_group(&["Day", "Week", "Month"], 0);
        assert_eq!(w.members.len(), 3);
        assert_eq!(w.labels, vec!["Day", "Week", "Month"]);
    }

    #[test]
    fn tap_reports_the_tapped_member_index() {
        let mut w = build_group(&["A", "B", "C"], 0);
        with_rects(&mut w, 3);
        let mut state = 0u32;
        // Tap the middle member (x in [40,80)).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0, 24.0));
        assert_eq!(w.active_member, Some(1));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 50.0, 24.0));
        assert_eq!(state, 1, "on_select fired with the tapped index");
    }

    #[test]
    fn is_controlled_selected_only_moves_via_rebuild() {
        // A tap reports the request but must NOT self-mutate `selected`.
        let mut w = build_group(&["A", "B"], 0);
        with_rects(&mut w, 2);
        let mut state = 0u32;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 50.0, 24.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 50.0, 24.0));
        assert_eq!(w.selected, 0, "widget did not self-mutate the selection");

        // The app feeds the confirmed selection back on the next rebuild.
        let next = button_group::<u32, _>(vec!["A", "B"], 1, |_s: &mut u32, _i| {});
        let prev = button_group::<u32, _>(vec!["A", "B"], 0, |_s: &mut u32, _i| {});
        let mut counter = 0u64;
        View::<u32>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.selected, 1, "selection moves on rebuild");
    }

    #[test]
    fn up_outside_the_pressed_member_does_not_fire() {
        let mut w = build_group(&["A", "B"], 0);
        with_rects(&mut w, 2);
        let mut state = 7u32;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 24.0)); // member 0
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 60.0, 24.0)); // out to member 1's area
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 60.0, 24.0));
        assert_eq!(state, 7, "release outside the pressed member must not fire");
    }

    #[test]
    fn down_outside_all_members_is_ignored() {
        let mut w = build_group(&["A", "B"], 0);
        with_rects(&mut w, 2);
        let mut state = 0u32;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(120.0, 48.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Down, 200.0, 24.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(w.active_member.is_none());
    }

    #[test]
    fn cancel_clears_the_press_without_firing() {
        let mut w = build_group(&["A", "B"], 0);
        with_rects(&mut w, 2);
        let mut state = 3u32;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 24.0));
        assert!(w.armed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 24.0));
        assert!(!w.armed);
        assert_eq!(state, 3);
    }

    /// A recording scene capturing filled/stroked paths (element counts only).
    #[derive(Default)]
    struct PathRecorder {
        fills: Vec<BezPath>,
        strokes: usize,
    }

    impl PaintScene for PathRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_path(&mut self, _o: Point, path: &BezPath, _b: &Brush) {
            self.fills.push(path.clone());
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
    }

    #[test]
    fn paints_one_background_path_per_member_when_unpressed() {
        let mut w = build_group(&["A", "B", "C"], 1);
        with_rects(&mut w, 3);
        let mut rec = PathRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 48.0));
        w.paint(&mut ctx, &mut rec);
        // Three member backgrounds, three outline strokes, no emphasis overlay.
        assert_eq!(rec.fills.len(), 3);
        assert_eq!(rec.strokes, 3);
    }

    #[test]
    fn pressing_adds_a_morphing_emphasis_path() {
        let mut w = build_group(&["A", "B"], 0);
        with_rects(&mut w, 2);
        let mut state = 0u32;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 24.0));
        // Seed then advance the spring so the emphasis has a non-zero t.
        w.press_anim.advance(FrameTime::ZERO);
        w.press_anim.advance(FrameTime::from_nanos(16_000_000));

        let mut rec = PathRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 48.0));
        w.paint(&mut ctx, &mut rec);
        // Two member backgrounds + one emphasis overlay = 3 filled paths.
        assert_eq!(
            rec.fills.len(),
            3,
            "the pressed member gets an emphasis path"
        );
        // The emphasis path is a closed shape_morph polygon.
        assert!(matches!(
            rec.fills[2].elements().last(),
            Some(PathEl::ClosePath)
        ));
    }

    #[test]
    fn themed_paint_resolves_container_and_shape_tokens() {
        let theme = Theme::m3_baseline();
        let mut w = build_group(&["A", "B"], 0);
        with_rects(&mut w, 2);

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
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 48.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        let scheme = theme.scheme();
        assert_eq!(
            rec.fills[0], scheme.secondary_container,
            "member 0 selected"
        );
        assert_eq!(rec.fills[1], scheme.surface, "member 1 unselected");
    }

    #[test]
    fn semantics_yields_a_radiogroup_of_members_with_selection() {
        fn logic(_s: &mut ()) -> ButtonGroupView<()> {
            button_group::<(), _>(vec!["One", "Two", "Three"], 2, |_s: &mut (), _i| {})
        }
        let mut root: frust_core::RenderRoot<(), ButtonGroupView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioGroup)
            .expect("a RadioGroup container node is contributed");
        assert_eq!(group.1.children().len(), 3);

        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::RadioButton)
            .collect();
        assert_eq!(buttons.len(), 3);
        let selected = buttons
            .iter()
            .find(|(_, n)| n.label() == Some("Three"))
            .expect("the Three member is present");
        assert_eq!(selected.1.is_selected(), Some(true));
        let unselected = buttons
            .iter()
            .find(|(_, n)| n.label() == Some("One"))
            .expect("the One member is present");
        assert_eq!(unselected.1.is_selected(), Some(false));
    }
}
