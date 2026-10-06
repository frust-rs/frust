//! Ports beUI's `shared-layout-bg` component.
//!
//! **Source:** `components/motion/shared-layout-bg.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. Registry
//! entry: slug `shared-layout-bg`, *"A pill that glides between hovered items
//! via motion's shared layout, with blur enter/exit."*
//!
//! | Upstream | Here |
//! |---|---|
//! | `as = "div" \| "ul"` | [`SharedLayoutBgKind`] |
//! | `inset = 20` | [`SharedLayoutBgView::inset`], [`DEFAULT_INSET`] |
//! | `bg-primary/[0.06] rounded-2xl` | [`DEFAULT_PILL_ALPHA`], [`style::RADIUS_2XL`] |
//! | `layoutId` + `SPRING_LAYOUT` | the sprung pill rect, [`SPRING_LAYOUT`] |
//! | `variants` opacity `0 → 1` | [`Presence`] on [`SHARED_LAYOUT_FADE`] |
//! | `onMouseEnter` per row / `onMouseLeave` on the list | the hover latch |
//!
//! # `layoutId` restated as a rect spring
//!
//! Upstream's pill is a Motion `layoutId` element: React unmounts it from the
//! row the pointer left and mounts a fresh one inside the row it arrived at, and
//! Motion's shared-layout projection makes the two look like one element gliding
//! between them. frust has no such projection — and no reconciliation event a
//! widget could observe if it did — so the pill here is a **single retained
//! rect** that springs from wherever it is displaying toward wherever it now
//! belongs, on the same [`SPRING_LAYOUT`] upstream hands its `transition`. The
//! visible result is the same glide; what is gone is the *mechanism*, and with
//! it upstream's `layoutRoot` scroll-smear guard, which has nothing to guard
//! against when there is no projection measuring page coordinates.
//!
//! Travel is driven from **raw** spring progress, so the pill passes its target
//! rect and settles back — that overshoot is what makes it read as a spring
//! rather than a slide. Opacity, being bounded, reads the clamped form.
//!
//! # Degradation: the enter/exit blur is an opacity fade
//!
//! Upstream's variants are `{ opacity: 0, filter: "blur(6px)" }` →
//! `{ opacity: 1, filter: "blur(0px)" }`. frust's scene has no filter primitive
//! at all — `PaintScene` can push a clip, a layer and a transform, and can draw
//! a *blurred rounded-rect shadow*, but nothing that blurs already-painted
//! content — so the blur is dropped and the fade kept. That is exactly what
//! upstream itself does under `useReducedMotion` (its `reducedVariants` are the
//! same variants with the filter removed), so the degradation has upstream's own
//! reduced-motion appearance rather than an invented one.
//!
//! # Enter, move, exit — three different animations
//!
//! Upstream's `exit` variant returns `{}` when another row is still active,
//! which is how it distinguishes *moving* from *leaving*. The three cases are
//! kept here:
//!
//! * **Enter** (nothing was highlighted): the pill appears at the target row's
//!   rect with no travel, and fades in.
//! * **Move** (a row was already highlighted): no fade at all; the rect springs.
//! * **Exit** (the pointer left the list): the pill holds its last rect and
//!   fades out.
//!
//! # A press keeps the highlight
//!
//! The framework's hover link ends at a pointer `Down`, and
//! `PaintCtx::is_hovered` — the authoritative read this widget re-syncs from
//! every paint — reports `false` for the whole of a press. A highlight that
//! honoured that would blink out the instant a user pressed the row they were
//! about to choose, which is the one moment it most needs to be visible. So this
//! component answers the substrate's press/hover question the same way
//! [`tilt_card`](super::tilt_card) does and the opposite way
//! [`marquee`](super::marquee) does: **a press keeps the highlight**, and the
//! paint-time correction is suspended while one is in flight.
//!
//! # Reuse
//!
//! The pill is deliberately not welded to a selection model: the highlight is
//! uncontrolled (upstream's own `useState`), and
//! [`SharedLayoutBgView::on_active_change`] reports it outward, so a tabs strip
//! or a menu can drive its own state from the same gesture without this
//! component learning what a tab is.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Rect, Role,
    SemanticsCtx, View, Widget, build_child, erase_callback_arg, rebuild_children, route_event,
    teardown_child, visit_children,
};
use frust::{FrameTime, Theme};
use kurbo::{Point, Size};
use peniko::Color;

use crate::motion::{Presence, Ramp};
use crate::press::{inside, presses};
use crate::style;
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};

/// How far the pill reaches past each row on both sides, in logical px —
/// upstream's `inset = 20` (`style={{ left: -inset, right: -inset }}`).
pub const DEFAULT_INSET: f64 = 20.0;

/// The pill's tint as a fraction of the primary role — upstream's
/// `bg-primary/[0.06]`.
pub const DEFAULT_PILL_ALPHA: f32 = 0.06;

/// The enter/exit fade.
///
/// Upstream's variants name no `transition`, so they run Motion's default
/// opacity tween — 0.3s. The curve is the catalog's own [`EASE_OUT`] rather than
/// Motion's default easing, which is the substitution every ported "no explicit
/// transition" animation in this catalog makes.
pub const SHARED_LAYOUT_FADE: Ramp = Ramp::eased(Duration::from_millis(300), EASE_OUT);

/// Unthemed fallback pill tint (beUI light `--primary`).
const FALLBACK_PRIMARY: Color = BEUI_LIGHT.primary;

/// What kind of container the rows sit in — upstream's `as` prop, which is
/// purely a semantic choice (both branches render the identical `flex w-full
/// flex-col`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SharedLayoutBgKind {
    /// `as="div"`: a plain grouping container.
    #[default]
    Block,
    /// `as="ul"`: a list, so assistive tech announces the row count.
    List,
}

impl SharedLayoutBgKind {
    /// Both, in upstream's own prop order.
    pub const ALL: [SharedLayoutBgKind; 2] = [SharedLayoutBgKind::Block, SharedLayoutBgKind::List];

    /// The semantics role this container publishes.
    pub fn role(self) -> Role {
        match self {
            SharedLayoutBgKind::Block => Role::Group,
            SharedLayoutBgKind::List => Role::List,
        }
    }
}

/// A view-held callback reporting which row is highlighted (erased on build).
type OnActiveChange<State> = Rc<dyn Fn(&mut State, Option<usize>)>;

/// A declarative beUI shared-layout background: a column of rows with one pill
/// that glides to whichever the pointer is over. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::shared_layout_bg::{SharedLayoutBgKind, shared_layout_bg};
///
/// let menu = shared_layout_bg::<()>(vec![text("one"), text("two")])
///     .kind(SharedLayoutBgKind::List)
///     .inset(12.0);
/// ```
pub struct SharedLayoutBgView<State: 'static> {
    rows: Vec<AnyView<State>>,
    kind: SharedLayoutBgKind,
    inset: f64,
    radius: f64,
    pill_alpha: f32,
    on_active_change: Option<OnActiveChange<State>>,
}

/// Stack `rows` in a column under one gliding highlight pill, with upstream's
/// own defaults.
pub fn shared_layout_bg<State: 'static>(
    rows: impl IntoIterator<Item = impl View<State>>,
) -> SharedLayoutBgView<State> {
    SharedLayoutBgView {
        rows: rows.into_iter().map(AnyView::new).collect(),
        kind: SharedLayoutBgKind::default(),
        inset: DEFAULT_INSET,
        radius: style::RADIUS_2XL,
        pill_alpha: DEFAULT_PILL_ALPHA,
        on_active_change: None,
    }
}

impl<State: 'static> SharedLayoutBgView<State> {
    /// Which container the rows sit in (`as`, default
    /// [`SharedLayoutBgKind::Block`]).
    pub fn kind(mut self, kind: SharedLayoutBgKind) -> Self {
        self.kind = kind;
        self
    }

    /// How far the pill reaches past each row on both sides (`inset`, default
    /// [`DEFAULT_INSET`]).
    pub fn inset(mut self, inset: f64) -> Self {
        self.inset = inset.max(0.0);
        self
    }

    /// The pill's corner radius (`rounded-2xl` = [`style::RADIUS_2XL`]).
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius.max(0.0);
        self
    }

    /// The pill's tint strength against the primary role (default
    /// [`DEFAULT_PILL_ALPHA`]) — upstream's `pillClassName` escape hatch,
    /// narrowed to the one value it exists to change.
    pub fn pill_alpha(mut self, alpha: f32) -> Self {
        self.pill_alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// Report which row is highlighted, or `None` once the pointer leaves.
    ///
    /// Upstream keeps `activeId` wholly private; this is the seam that makes the
    /// component reusable by a tabs strip or a menu (see the [module
    /// docs](self)). The highlight itself stays uncontrolled either way.
    pub fn on_active_change<F: Fn(&mut State, Option<usize>) + 'static>(
        mut self,
        callback: F,
    ) -> Self {
        self.on_active_change = Some(Rc::new(callback));
        self
    }
}

/// The retained widget for a [`SharedLayoutBgView`].
pub struct SharedLayoutBgWidget {
    rows: Vec<ChildPod>,
    kind: SharedLayoutBgKind,
    inset: f64,
    radius: f64,
    pill_alpha: f32,
    /// The highlighted row, latched from pointer moves and self-corrected from
    /// `PaintCtx::is_hovered` — upstream's `activeId`.
    active: Option<usize>,
    /// Whether a press is in flight, which suspends the paint-time correction.
    /// See the [module docs](self).
    pressed: bool,
    /// The enter/exit fade — upstream's `AnimatePresence` wrapper.
    presence: Presence,
    /// The rect the current travel started from; `None` when the pill is resting
    /// on its target or appearing fresh.
    pill_from: Option<Rect>,
    /// The rect the pill painted last frame — the retarget origin, and what an
    /// exiting pill keeps painting while it fades.
    pill_shown: Option<Rect>,
    /// The frame the current travel started on.
    pill_started: Option<FrameTime>,
    on_active_change: Option<ErasedArgCallback<Option<usize>>>,
}

impl SharedLayoutBgWidget {
    /// The highlighted row, if any.
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// The rect the pill painted on the last paint, in the container's own
    /// space — `None` while it is wholly absent.
    pub fn pill_shown(&self) -> Option<Rect> {
        self.pill_shown
    }

    /// Row `index`'s laid-out box, in the container's own space.
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        self.rows
            .get(index)
            .map(|pod| Rect::from_origin_size(pod.origin(), pod.size()))
    }

    /// Where the pill belongs for row `index`: the row's box, reaching
    /// [`SharedLayoutBgView::inset`] past it on each side (`inset-y-0` with
    /// `left: -inset, right: -inset`).
    pub fn pill_rect_for(&self, index: usize) -> Option<Rect> {
        self.row_rect(index)
            .map(|rect| Rect::new(rect.x0 - self.inset, rect.y0, rect.x1 + self.inset, rect.y1))
    }

    /// The row under a container-local `pos`, if any.
    fn hit_row(&self, pos: Point) -> Option<usize> {
        (0..self.rows.len())
            .find(|index| self.row_rect(*index).is_some_and(|rect| rect.contains(pos)))
    }

    /// Move the highlight to `next`, staging the pill's travel. Returns whether
    /// anything changed.
    ///
    /// A move *between* rows launches the rect spring from wherever the pill is
    /// displaying; an arrival from nothing does not (it appears at the target and
    /// fades in), and a departure does not (it holds its rect and fades out) —
    /// the three cases the module docs set out.
    fn set_active(&mut self, next: Option<usize>) -> bool {
        if self.active == next {
            return false;
        }
        if next.is_some() && self.presence.is_visible() {
            self.pill_from = self.pill_shown;
            self.pill_started = None;
        }
        self.active = next;
        true
    }

    /// Report the highlight outward, if a consumer asked for it.
    fn emit(&mut self, ctx: &mut EventCtx) {
        let active = self.active;
        if let Some(callback) = &mut self.on_active_change {
            callback(ctx, active);
        }
    }

    /// Advance the pill's travel to `now`, returning the rect to paint and
    /// whether it is still moving.
    fn advance_pill(&mut self, now: FrameTime, reduce: bool) -> (Option<Rect>, bool) {
        let Some(target) = self.active.and_then(|index| self.pill_rect_for(index)) else {
            // Exiting: hold the last rect while the fade runs.
            return (self.pill_shown, false);
        };
        let Some(from) = self.pill_from.filter(|_| !reduce) else {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        };
        let ramp = Ramp::spring(SPRING_LAYOUT);
        let started = *self.pill_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        }
        // Raw progress: passing the target rect and settling back is the point.
        let shown = lerp_rect(from, target, ramp.progress(elapsed));
        self.pill_shown = Some(shown);
        (Some(shown), true)
    }
}

impl<State: 'static> View<State> for SharedLayoutBgView<State> {
    type Element = SharedLayoutBgWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SharedLayoutBgWidget {
        SharedLayoutBgWidget {
            rows: self.rows.iter().map(|row| build_child(row, ctx)).collect(),
            kind: self.kind,
            inset: self.inset,
            radius: self.radius,
            pill_alpha: self.pill_alpha,
            active: None,
            pressed: false,
            presence: Presence::symmetric(SHARED_LAYOUT_FADE),
            pill_from: None,
            pill_shown: None,
            pill_started: None,
            on_active_change: self.on_active_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SharedLayoutBgWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_active_change = self.on_active_change.as_ref().map(erase_callback_arg);
        let mut flags = rebuild_children(
            &prev.rows,
            &self.rows,
            &mut element.rows,
            ctx,
            |view| view,
            |_| None,
        );
        if prev.rows.len() != self.rows.len() {
            // The row the highlight named may not exist any more, and a pill
            // launched from a rect that no longer means anything would glide
            // from nowhere.
            element.active = None;
            element.pill_from = None;
            element.pill_started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.kind != self.kind {
            element.kind = self.kind;
            // Semantics-only, but `PAINT` is what bumps the root's semantics
            // dirty gate and there is no narrower flag.
            flags |= ChangeFlags::PAINT;
        }
        if element.inset != self.inset {
            element.inset = self.inset;
            flags |= ChangeFlags::PAINT;
        }
        if element.radius != self.radius {
            element.radius = self.radius;
            flags |= ChangeFlags::PAINT;
        }
        if element.pill_alpha != self.pill_alpha {
            element.pill_alpha = self.pill_alpha;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SharedLayoutBgWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.rows.iter().zip(element.rows.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for SharedLayoutBgWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // `flex w-full flex-col`: every row takes the container's full width and
        // stacks under the last, with no gap (upstream sets none).
        let room = bc.max();
        let width = if room.width.is_finite() {
            room.width
        } else {
            0.0
        };
        let row_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        let mut y = 0.0_f64;
        let mut widest = 0.0_f64;
        for pod in &mut self.rows {
            let size = pod.layout_child(ctx, &row_bc);
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
            widest = widest.max(size.width);
        }
        bc.constrain(Size::new(width.max(widest), y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let tint = theme.map_or(FALLBACK_PRIMARY, |t| t.scheme().primary);
        let origin = ctx.origin();
        let now = ctx.frame_time();

        // The authoritative hover answer, except while a press owns the list.
        if !self.pressed && !ctx.is_hovered() {
            self.set_active(None);
        }

        // Upstream's `reducedVariants` keep the fade and drop only the blur, so
        // `reduce_motion` collapses the *travel* here, not the presence.
        let staged = self.presence.set_open(self.active.is_some());
        let mut owes_frame = staged;
        let alpha = self.presence.advance(now).clamp(0.0, 1.0);
        owes_frame |= self.presence.is_animating();
        let (rect, moving) = self.advance_pill(now, reduce);
        owes_frame |= moving;

        if !self.presence.is_visible() {
            // Fully exited: forget the rect so the next arrival enters fresh
            // rather than gliding in from where the last one died.
            self.pill_shown = None;
            self.pill_from = None;
        } else if let Some(rect) = rect
            && alpha > 0.0
        {
            // `pointer-events-none absolute inset-y-0`, painted under the rows
            // so a row's own ink stays on top (`relative z-10`).
            scene.push_layer(origin, ctx.size(), alpha as f32);
            scene.fill_rounded_rect(
                origin + rect.origin().to_vec2(),
                rect.size(),
                self.radius,
                style::with_alpha(tint, self.pill_alpha),
            );
            scene.pop_layer();
        }

        for pod in &mut self.rows {
            pod.paint_child(ctx, scene);
        }

        if owes_frame {
            // A fade and a glide both have visible endpoints, so an unpaced
            // frame is the right ask.
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Rows see everything first: the highlight is decoration around whatever
        // they are, and never swallows an interaction meant for one.
        let routed = route_event(&mut self.rows, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        let InputEvent::Pointer(pointer) = event else {
            return routed;
        };
        let size = ctx.size();
        match pointer.phase {
            PointerPhase::Down if presses(pointer) => self.pressed = true,
            PointerPhase::Up | PointerPhase::Cancel => self.pressed = false,
            _ => {}
        }
        if pointer.phase != PointerPhase::Move {
            return routed;
        }
        let next = if inside(pointer.position, size) {
            ctx.claim_hover();
            self.hit_row(pointer.position)
        } else {
            // Upstream's `onMouseLeave` on the container itself.
            None
        };
        if self.set_active(next) {
            self.emit(ctx);
            ctx.request_redraw();
        }
        routed
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The pill is `pointer-events-none` decoration; only the rows are real.
        ctx.push_container(
            self.kind.role(),
            |_| {},
            |ctx| {
                for pod in &self.rows {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(rows);
}

/// Interpolate one rect toward another, corner by corner — the rect equivalent
/// of a scalar lerp, and what makes a `layoutId` glide expressible as one
/// retained element.
fn lerp_rect(from: Rect, to: Rect, t: f64) -> Rect {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x0, to.x0),
        lerp(from.y0, to.y0),
        lerp(from.x1, to.x1),
        lerp(from.y1, to.y1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use frust::{SizedBox, any};
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The list every test lays out: three 30px rows in a 200px-wide column.
    const BOX: Size = Size::new(200.0, 90.0);
    /// One row's height.
    const ROW: f64 = 30.0;

    /// Records the rounded rects the pill painted and the layer alphas it was
    /// composited at.
    #[derive(Default)]
    struct Recorder {
        pills: Vec<(Point, Size, f64, Color)>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.pills.push((origin, size, radius, color));
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    fn rows() -> Vec<frust::AnyView<()>> {
        (0..3)
            .map(|_| any(SizedBox::<()>(Some(BOX.width), Some(ROW))))
            .collect()
    }

    fn laid_out(view: &SharedLayoutBgView<()>) -> SharedLayoutBgWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(BOX));
        widget
    }

    fn painted(
        widget: &mut SharedLayoutBgWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn send(widget: &mut SharedLayoutBgWidget, phase: PointerPhase, x: f64, y: f64) -> bool {
        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            }),
        );
        ctx.needs_redraw()
    }

    /// Put the pointer on the row at `y` **and hold a press**, so the paints a
    /// test drives afterward do not clear the highlight.
    ///
    /// `PaintCtx::for_test` has no seam for seeding the hover flag, so every
    /// test paint reports "not hovered"; the pressed carve-out this component
    /// documents is the only way to observe a *live* highlight across a paint.
    fn engage(widget: &mut SharedLayoutBgWidget, y: f64) {
        send(widget, PointerPhase::Down, 100.0, y);
        send(widget, PointerPhase::Move, 100.0, y);
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The rows stack full-width with no gap, and the pill reaches `inset` past
    /// each one on both sides while keeping its height.
    #[test]
    fn the_pill_reaches_past_each_row_by_the_inset() {
        let widget = laid_out(&shared_layout_bg(rows()));
        assert_eq!(
            widget.row_rect(1),
            Some(Rect::new(0.0, ROW, BOX.width, ROW * 2.0))
        );
        assert_eq!(
            widget.pill_rect_for(1),
            Some(Rect::new(
                -DEFAULT_INSET,
                ROW,
                BOX.width + DEFAULT_INSET,
                ROW * 2.0
            ))
        );

        // The inset is the one dimension it changes.
        let tight = laid_out(&shared_layout_bg(rows()).inset(0.0));
        assert_eq!(tight.pill_rect_for(1), tight.row_rect(1));
    }

    /// Hovering a row highlights it; the pointer moving off the list clears it,
    /// and the paint-time sync is authoritative over any latched flag.
    #[test]
    fn hovering_a_row_highlights_it_and_leaving_clears_it() {
        let mut widget = laid_out(&shared_layout_bg(rows()));
        assert_eq!(widget.active(), None);

        assert!(send(&mut widget, PointerPhase::Move, 100.0, 45.0));
        assert_eq!(widget.active(), Some(1), "the middle row");

        assert!(send(&mut widget, PointerPhase::Move, 100.0, 75.0));
        assert_eq!(widget.active(), Some(2));

        // A move off the list is upstream's `onMouseLeave`.
        assert!(send(&mut widget, PointerPhase::Move, 400.0, 75.0));
        assert_eq!(widget.active(), None);

        // …and `PaintCtx::is_hovered` catches a pointer that left silently.
        send(&mut widget, PointerPhase::Move, 100.0, 15.0);
        assert_eq!(widget.active(), Some(0));
        painted(&mut widget, 0, None);
        assert_eq!(widget.active(), None, "paint did not re-sync the latch");
    }

    /// A press keeps the highlight, which is the settled rule for a selection
    /// affordance and the opposite of `marquee`'s.
    #[test]
    fn a_press_keeps_the_highlight() {
        let mut widget = laid_out(&shared_layout_bg(rows()));
        send(&mut widget, PointerPhase::Move, 100.0, 45.0);
        assert_eq!(widget.active(), Some(1));

        send(&mut widget, PointerPhase::Down, 100.0, 45.0);
        painted(&mut widget, 0, None);
        assert_eq!(
            widget.active(),
            Some(1),
            "the paint-time sync cleared a pressed row"
        );

        // Releasing hands the list back to the ordinary hover rule.
        send(&mut widget, PointerPhase::Up, 100.0, 45.0);
        painted(&mut widget, 10, None);
        assert_eq!(widget.active(), None);
    }

    /// The three cases the module docs set out: an arrival places the pill and
    /// fades it in, a move springs the rect with no fade, and a departure holds
    /// the rect while it fades out.
    #[test]
    fn entering_moving_and_leaving_animate_differently() {
        let mut widget = laid_out(&shared_layout_bg(rows()));

        // Enter: no travel, and the fade starts from nothing — a fully clear
        // pill paints nothing at all.
        engage(&mut widget, 15.0);
        let (recorder, needs_frame) = painted(&mut widget, 0, None);
        assert!(needs_frame, "the enter fade owes frames");
        assert_eq!(widget.pill_shown(), widget.pill_rect_for(0));
        assert!(recorder.pills.is_empty(), "the pill entered at full alpha");

        // Part-way through, it is visibly fading up.
        let (recorder, _) = painted(&mut widget, 150, None);
        let rising = recorder.layers[0];
        assert!(rising > 0.0 && rising < 1.0, "no enter fade: {rising}");

        // Let the fade land.
        let (recorder, _) = painted(&mut widget, 500, None);
        assert_eq!(recorder.layers, vec![1.0]);

        // Move: the pill leaves the first row's rect and is somewhere between
        // the two, at full opacity throughout.
        send(&mut widget, PointerPhase::Move, 100.0, 75.0);
        // The first paint after a retarget only latches the travel's start
        // frame, so it still shows the origin rect; the next one has moved.
        painted(&mut widget, 510, None);
        let (recorder, needs_frame) = painted(&mut widget, 560, None);
        assert!(needs_frame, "the glide owes frames");
        assert_eq!(recorder.layers, vec![1.0], "a move must not re-fade");
        let shown = widget.pill_shown().expect("the pill is visible");
        let from = widget.pill_rect_for(0).unwrap();
        let to = widget.pill_rect_for(2).unwrap();
        assert!(
            shown.y0 > from.y0 && shown.y0 < to.y0,
            "the pill did not glide: {shown:?}"
        );

        // …and it arrives.
        for step in 0..40 {
            painted(&mut widget, 580 + step * 20, None);
        }
        assert_eq!(widget.pill_shown(), Some(to));

        // Leave: the rect is held while the opacity runs back down.
        send(&mut widget, PointerPhase::Up, 100.0, 75.0);
        send(&mut widget, PointerPhase::Move, 400.0, 75.0);
        painted(&mut widget, 1_400, None);
        let (recorder, needs_frame) = painted(&mut widget, 1_500, None);
        assert!(needs_frame, "the exit fade owes frames");
        assert_eq!(widget.pill_shown(), Some(to), "the rect moved on exit");
        let fading = recorder.layers[0];
        assert!(
            fading > 0.0 && fading < 1.0,
            "the exit did not fade: {fading}"
        );

        // Once gone it is gone, and asks for nothing further.
        let (recorder, needs_frame) = painted(&mut widget, 2_000, None);
        assert!(!needs_frame);
        assert!(recorder.pills.is_empty());
        assert_eq!(widget.pill_shown(), None);
    }

    /// The pill is painted with the primary role at upstream's own alpha and the
    /// `rounded-2xl` radius, under the rows rather than over them.
    #[test]
    fn the_pill_paints_a_primary_tint_beneath_the_rows() {
        let mut widget = laid_out(&shared_layout_bg(rows()).pill_alpha(0.5));
        engage(&mut widget, 45.0);
        painted(&mut widget, 0, None);
        let (recorder, _) = painted(&mut widget, 400, None);

        assert_eq!(recorder.pills.len(), 1);
        let (at, size, radius, colour) = recorder.pills[0];
        assert_eq!(at, Point::new(-DEFAULT_INSET, ROW));
        assert_eq!(size, Size::new(BOX.width + DEFAULT_INSET * 2.0, ROW));
        assert_eq!(radius, style::RADIUS_2XL);
        assert!((colour.components[3] - 0.5).abs() < 1e-6);
    }

    /// `reduce_motion` drops the *travel* and keeps the fade — upstream's own
    /// `reducedVariants`, which remove the blur and nothing else, plus its
    /// `{ duration: 0 }` layout transition.
    #[test]
    fn reduce_motion_jumps_the_pill_but_keeps_the_fade() {
        let theme = reduced();
        let mut widget = laid_out(&shared_layout_bg(rows()));
        engage(&mut widget, 15.0);
        painted(&mut widget, 0, Some(&theme));
        painted(&mut widget, 400, Some(&theme));

        // A move lands on the new rect immediately rather than gliding.
        send(&mut widget, PointerPhase::Move, 100.0, 75.0);
        let (recorder, _) = painted(&mut widget, 410, Some(&theme));
        assert_eq!(widget.pill_shown(), widget.pill_rect_for(2));

        // …and the fade still ran on the way in.
        assert_eq!(recorder.layers, vec![1.0]);
    }

    /// An empty list highlights nothing, paints nothing and asks for nothing.
    #[test]
    fn an_empty_list_is_inert() {
        let mut widget = laid_out(&shared_layout_bg::<()>(Vec::<AnyView<()>>::new()));
        assert!(!send(&mut widget, PointerPhase::Move, 10.0, 10.0));
        assert_eq!(widget.active(), None);
        let (recorder, needs_frame) = painted(&mut widget, 0, None);
        assert!(!needs_frame);
        assert!(recorder.pills.is_empty());
    }

    /// Both container kinds are constructible and publish their own role.
    #[test]
    fn every_kind_is_constructible() {
        assert_eq!(SharedLayoutBgKind::ALL.len(), 2);
        for kind in SharedLayoutBgKind::ALL {
            let widget = laid_out(&shared_layout_bg(rows()).kind(kind));
            assert_eq!(widget.kind.role(), kind.role());
        }
        assert_eq!(SharedLayoutBgKind::Block.role(), Role::Group);
        assert_eq!(SharedLayoutBgKind::List.role(), Role::List);
    }
}
