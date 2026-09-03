//! Ports beUI's `tilt-card` component.
//!
//! **Source:** `components/motion/tilt-card.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. Registry
//! entry: slug `tilt-card`, *"3D perspective tilt on hover with cursor-tracked
//! glare."*
//!
//! | Upstream | Here |
//! |---|---|
//! | `max = 12` | [`TiltCardView::max`], [`DEFAULT_MAX_TILT`] |
//! | `glare = true` | [`TiltCardView::glare`] |
//! | `rx`/`ry` from the cursor fraction | [`TiltCardWidget::tilt`], from `PointerTracker::offset` |
//! | `useSpring(…, SPRING_MOUSE)` | two spring scalars on [`SPRING_MOUSE`] |
//! | `rounded-2xl overflow-hidden` | a [`style::RADIUS_2XL`] rounded clip |
//! | `useReducedMotion() \|\| !canHover` → effect off | the theme's `reduce_motion` |
//!
//! # The 3D-perspective degradation
//!
//! Upstream's transform is `perspective(1000px) rotateX(rx) rotateY(ry)` on a
//! `transform-style: preserve-3d` element. That is a genuine projective
//! transform: it maps the card's rectangle onto a **trapezoid**, the edge
//! rotating away narrowing while the edge rotating toward the viewer widens.
//!
//! frust's scene has one transform primitive, `PaintScene::push_transform`, and
//! it takes an [`Affine`] — a 2x3 matrix, which by construction keeps parallel
//! lines parallel and therefore cannot produce a trapezoid at all. There is no
//! projective seam anywhere below it. So the tilt here is the **affine shadow**
//! of the same two rotations, composed about the card's centre from the two
//! effects an affine can carry:
//!
//! * **Foreshortening.** A face rotated by `θ` about an in-plane axis presents
//!   `cos θ` of its extent across that axis, so `rotateY(ry)` becomes a
//!   horizontal scale of `cos ry` and `rotateX(rx)` a vertical scale of
//!   `cos rx`. This part is *exact* — it is what the projection does to the
//!   card's mid-line — and it is the whole of the effect at the centre of the
//!   travel, where a tilt spends most of its time.
//! * **Shear.** The trapezoid's leading edge also rides up or down relative to
//!   the trailing one. A single shear per axis, `sin θ ·` [`TILT_SHEAR`], keeps
//!   that read of a lifted corner; the second-order flare that separates a
//!   shear from a true perspective divide is what is lost.
//!
//! What that costs, stated plainly: at the default `max = 12°` the two
//! transforms are visually close (`cos 12° ≈ 0.978`, so the foreshortening is a
//! ~2% squeeze either way and the shear carries the rest). At a large `max` the
//! affine reads as a *lean* where upstream reads as a *rotation into depth* —
//! the near corner does not grow. [`TiltCardView::shadow`] is the compensation
//! offered for it, and is **off by default** because upstream paints no shadow
//! at all: a card lifted toward the pointer casting its blurred elevation the
//! other way restores some of the depth cue the projection carried for free.
//! Turning it on is a deliberate departure from upstream, not a port.
//!
//! # A press keeps the tilt
//!
//! The catalog's substrate deliberately leaves the press/hover question to each
//! component ([`crate::motion::pointer`]), and a cursor-follow answers it the
//! opposite way to [`marquee`](super::marquee)'s pause-on-hover: **a press keeps
//! the offset.** The framework's hover link ends at a pointer `Down`, so
//! `PaintCtx::is_hovered` reads `false` for the whole of a press; a tilt that
//! honoured it would snap flat the instant a user pressed the card and spring
//! back up on release, which is the opposite of a surface that follows the
//! cursor. So the paint-time `PointerTracker::sync_hovered` correction — the
//! authoritative one for every other consumer — is **skipped while pressed**,
//! and `Down`/`Move` both track.
//!
//! A release does return the card to rest: the tracker resets on `Up`/`Cancel`
//! (holding a position across one would put it straight back into disagreement
//! with the next paint's sync), and the next uncaptured `Move` re-engages the
//! tilt. That is the one place the port visibly differs from upstream, whose
//! `onMouseLeave` is the only reset it has.
//!
//! # Touch and reduced motion
//!
//! Upstream gates the whole effect on `useReducedMotion()` **and**
//! `useHoverCapable()`, skipping it on a touch device because a tap manufactures
//! a phantom hover. frust's `PointerEvent` draws no touch/mouse distinction, so
//! only the first gate has a counterpart here: under the theme's
//! `motion.reduce_motion` the card paints flat, tracks nothing and asks for no
//! frames. The touch guard is inherited from the framework instead — a captured
//! pointer creates no hover, and this widget captures nothing.

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
    build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{FrameTime, Theme};
use kurbo::{Point, Size, Vec2};
use peniko::{Brush, Color, ColorStop, Gradient};

use crate::motion::{PointerTracker, Ramp};
use crate::press::{SpringScalar, inside, presses};
use crate::style;
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::SPRING_MOUSE;

/// The tilt's travel in degrees — upstream's `max = 12`. The card reaches
/// `±max/2` on each axis at the box's edges, because upstream's `(px − 0.5)`
/// factor runs `−0.5..0.5` rather than `−1..1`.
pub const DEFAULT_MAX_TILT: f64 = 12.0;

/// How much of each rotation becomes shear rather than foreshortening — the one
/// authored number in the affine approximation (see the [module docs](self)).
///
/// Chosen so that at the default [`DEFAULT_MAX_TILT`] the corner displacement is
/// roughly a fortieth of the card's extent, the same order as the corner lift a
/// `perspective(1000px)` divide produces on a card a few hundred px wide.
pub const TILT_SHEAR: f64 = 0.35;

/// The glare's composited opacity — upstream's `opacity-15`.
pub const GLARE_OPACITY: f32 = 0.15;

/// Where the glare's radial gradient reaches full transparency, as a fraction of
/// the box's half-diagonal — upstream's `transparent 50%` against a
/// farthest-corner gradient box.
pub const GLARE_FALLOFF: f64 = 0.5;

/// The elevation shadow's blur radius when [`TiltCardView::shadow`] is on, in
/// logical px.
pub const SHADOW_BLUR: f64 = 24.0;

/// How far the elevation shadow slides away from the lift at full tilt, in
/// logical px.
pub const SHADOW_TRAVEL: f64 = 12.0;

/// The elevation shadow's alpha at full tilt.
pub const SHADOW_ALPHA: f32 = 0.18;

/// Unthemed fallback ink (beUI light `--foreground`) — the glare's own colour,
/// upstream's `var(--foreground)`.
const FALLBACK_FOREGROUND: Color = BEUI_LIGHT.foreground;

/// Unthemed fallback shadow colour.
const FALLBACK_SHADOW: Color = Color::BLACK;

/// A declarative beUI tilt card: a surface that leans toward the pointer and
/// carries a cursor-tracked glare. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::tilt_card::{TiltCardView, tilt_card};
///
/// let card: TiltCardView<()> = tilt_card(text("hover me")).max(18.0).glare(false);
/// ```
pub struct TiltCardView<State: 'static> {
    child: AnyView<State>,
    max: f64,
    glare: bool,
    shadow: bool,
    radius: f64,
}

/// Wrap `child` in a tilt card with upstream's own defaults: [`DEFAULT_MAX_TILT`]
/// of travel and the glare on.
pub fn tilt_card<State: 'static, V: View<State>>(child: V) -> TiltCardView<State> {
    TiltCardView {
        child: any(child),
        max: DEFAULT_MAX_TILT,
        glare: true,
        shadow: false,
        radius: style::RADIUS_2XL,
    }
}

impl<State: 'static> TiltCardView<State> {
    /// The tilt's travel in degrees (`max`, default [`DEFAULT_MAX_TILT`]).
    /// Negative values are clamped to zero — a card that leans *away* from the
    /// pointer is not a variant upstream offers.
    pub fn max(mut self, max: f64) -> Self {
        self.max = max.max(0.0);
        self
    }

    /// Paint the cursor-tracked glare (`glare`, default `true`, upstream's own
    /// default).
    pub fn glare(mut self, glare: bool) -> Self {
        self.glare = glare;
        self
    }

    /// Cast a blurred elevation shadow that slides opposite the tilt.
    ///
    /// **Off by default, and not a port**: upstream paints no shadow. It is the
    /// compensation offered for the depth cue an affine tilt cannot carry — see
    /// the [module docs](self).
    pub fn shadow(mut self, shadow: bool) -> Self {
        self.shadow = shadow;
        self
    }

    /// The corner radius the card clips its child to (`rounded-2xl` =
    /// [`style::RADIUS_2XL`]).
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius.max(0.0);
        self
    }
}

/// The retained widget for a [`TiltCardView`].
pub struct TiltCardWidget {
    child: ChildPod,
    max: f64,
    glare: bool,
    shadow: bool,
    radius: f64,
    /// Widget-local pointer state — fed from `Down`/`Move`, corrected from
    /// `PaintCtx::is_hovered` on every paint the card is *not* pressed for.
    pointer: PointerTracker,
    /// Whether a press is in flight, which suspends the paint-time hover
    /// correction. See the [module docs](self).
    pressed: bool,
    /// The rotation about the horizontal axis, in degrees, following the pointer
    /// on [`SPRING_MOUSE`] — upstream's `srx`.
    tilt_x: SpringScalar,
    /// The rotation about the vertical axis, in degrees — upstream's `sry`.
    tilt_y: SpringScalar,
}

impl TiltCardWidget {
    /// The sprung rotations `(rx, ry)` in degrees, as of the last paint.
    pub fn tilt(&self) -> (f64, f64) {
        (self.tilt_x.value(), self.tilt_y.value())
    }

    /// Where the glare's centre sits, in the card's own logical-px space —
    /// upstream's `gx`/`gy`, which are raw motion values rather than sprung
    /// ones, so this follows the pointer with no lag while the tilt trails it.
    pub fn glare_centre(&self) -> Point {
        self.pointer.position()
    }

    /// Whether the card is tracking a pointer at all: a press counts, a bare
    /// hover counts, and rest does not.
    pub fn is_engaged(&self) -> bool {
        self.pointer.hovered()
    }

    /// The rotations the current pointer offset asks for, in degrees.
    ///
    /// Upstream reads the cursor as a `0..1` fraction of the box and writes
    /// `ry = (px − 0.5)·max`, `rx = (0.5 − py)·max`. `PointerTracker::offset` is
    /// the same quantity doubled (`−1..1`), so each is halved back — the mapping
    /// is exact, not an approximation.
    pub fn target_tilt(&self) -> (f64, f64) {
        if !self.pointer.hovered() {
            return (0.0, 0.0);
        }
        let offset = self.pointer.offset();
        (-offset.y * self.max / 2.0, offset.x * self.max / 2.0)
    }

    /// The affine standing in for `perspective(1000px) rotateX(rx) rotateY(ry)`,
    /// composed about the box's centre. See the [module docs](self) for what it
    /// keeps and what it drops.
    pub fn tilt_transform(&self, size: Size) -> Affine {
        let (rx, ry) = self.tilt();
        let (rx, ry) = (rx.to_radians(), ry.to_radians());
        let centre = Vec2::new(size.width / 2.0, size.height / 2.0);
        Affine::translate(centre)
            * Affine::skew(-rx.sin() * TILT_SHEAR, ry.sin() * TILT_SHEAR)
            * Affine::scale_non_uniform(ry.cos(), rx.cos())
            * Affine::translate(-centre)
    }

    /// Re-aim both springs at the pointer's current ask. Returns whether either
    /// moved.
    fn retarget(&mut self) -> bool {
        let (rx, ry) = self.target_tilt();
        let moved_x = self.tilt_x.set_target(rx);
        let moved_y = self.tilt_y.set_target(ry);
        moved_x || moved_y
    }

    /// Step both springs to `now`, returning whether either still owes a frame.
    fn advance(&mut self, now: FrameTime) -> bool {
        self.tilt_x.advance(now);
        self.tilt_y.advance(now);
        self.tilt_x.is_animating() || self.tilt_y.is_animating()
    }

    /// Drop flat with no motion — the `reduce_motion` collapse.
    fn flatten(&mut self) {
        self.pointer.reset();
        self.pressed = false;
        self.tilt_x.jump_to(0.0);
        self.tilt_y.jump_to(0.0);
    }
}

impl<State: 'static> View<State> for TiltCardView<State> {
    type Element = TiltCardWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TiltCardWidget {
        let ramp = Ramp::spring(SPRING_MOUSE);
        TiltCardWidget {
            child: build_child(&self.child, ctx),
            max: self.max,
            glare: self.glare,
            shadow: self.shadow,
            radius: self.radius,
            pointer: PointerTracker::new(),
            pressed: false,
            tilt_x: SpringScalar::new(0.0, ramp),
            tilt_y: SpringScalar::new(0.0, ramp),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TiltCardWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.max != self.max {
            element.max = self.max;
            // The travel changed under a live pointer; re-aim rather than wait
            // for the next move.
            element.retarget();
            flags |= ChangeFlags::PAINT;
        }
        if element.glare != self.glare {
            element.glare = self.glare;
            flags |= ChangeFlags::PAINT;
        }
        if element.shadow != self.shadow {
            element.shadow = self.shadow;
            flags |= ChangeFlags::PAINT;
        }
        if element.radius != self.radius {
            element.radius = self.radius;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TiltCardWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for TiltCardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A transparent wrapper: the child sizes the card, exactly as
        // upstream's `div` wraps whatever it is given.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        self.pointer.set_size(size);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink = theme.map_or(FALLBACK_FOREGROUND, |t| t.scheme().on_surface);
        let shadow_ink = theme.map_or(FALLBACK_SHADOW, |t| t.scheme().shadow);
        let origin = ctx.origin();
        let size = ctx.size();
        self.pointer.set_size(size);

        if reduce {
            // Upstream's `enabled` gate: the whole effect is off, and the card
            // is a plain clipped surface.
            self.flatten();
            scene.push_clip_rounded(origin, size, self.radius);
            self.child.paint_child(ctx, scene);
            scene.pop_clip();
            return;
        }

        // The authoritative hover answer, except while a press owns the card —
        // see the module docs.
        if !self.pressed {
            self.pointer.sync_hovered(ctx.is_hovered());
        }
        self.retarget();
        let animating = self.advance(ctx.frame_time());

        let (rx, ry) = self.tilt();
        let reach = if self.max > 0.0 { self.max / 2.0 } else { 1.0 };
        let lean = ((rx.abs() + ry.abs()) / (2.0 * reach)).clamp(0.0, 1.0);
        if self.shadow && lean > 0.0 {
            // Away from the lift: a card leaning its top edge toward the viewer
            // throws its shadow down. A flat card casts nothing at all, so a
            // resting surface is pixel-identical with the affordance on or off.
            let slide = Vec2::new(-ry / reach * SHADOW_TRAVEL, rx / reach * SHADOW_TRAVEL);
            scene.draw_shadow(
                origin + slide,
                size,
                self.radius,
                SHADOW_BLUR,
                style::with_alpha(shadow_ink, SHADOW_ALPHA * lean as f32),
            );
        }

        // `overflow-hidden rounded-2xl`, pushed *under* the tilt so the clip
        // leans with the card rather than staying square around it.
        scene.push_transform(self.tilt_transform(size));
        scene.push_clip_rounded(origin, size, self.radius);
        self.child.paint_child(ctx, scene);
        if self.glare {
            paint_glare(scene, origin, size, self.glare_centre(), ink);
        }
        scene.pop_clip();
        scene.pop_transform();

        if animating {
            // A settling cursor-follow: a transition with a visible endpoint,
            // so an unpaced frame is the right ask.
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The child sees everything first — a tilt card is decoration around
        // whatever it wraps, and never swallows an interaction meant for it.
        let routed = route_event_single(&mut self.child, ctx, event);
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
        if pointer.phase == PointerPhase::Move && !self.pressed && inside(pointer.position, size) {
            ctx.claim_hover();
        }
        if self.pointer.on_pointer(pointer, size) {
            self.retarget();
            ctx.request_redraw();
        }
        // Watching a pointer is not consuming it: whatever the child made of
        // the event stands.
        routed
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Wholly decorative — upstream's wrapper is a bare `div` and its glare
        // is `aria-hidden`, so only the child reaches the tree.
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

/// Paint the cursor-tracked glare: upstream's
/// `radial-gradient(circle at gx% gy%, var(--foreground), transparent 50%)`
/// composited at [`GLARE_OPACITY`].
///
/// The gradient's radius is the box's half-diagonal (CSS's `farthest-corner`
/// default) scaled by [`GLARE_FALLOFF`], which is where upstream's second stop
/// reaches transparent.
fn paint_glare(scene: &mut dyn PaintScene, origin: Point, size: Size, centre: Point, ink: Color) {
    let radius = (size.width.hypot(size.height) / 2.0) * GLARE_FALLOFF;
    if radius <= 0.0 {
        return;
    }
    let gradient = Gradient::new_radial(origin + centre.to_vec2(), radius as f32).with_stops(
        [
            ColorStop::from((0.0f32, ink)),
            ColorStop::from((1.0f32, style::with_alpha(ink, 0.0))),
        ]
        .as_slice(),
    );
    scene.push_layer(origin, size, GLARE_OPACITY);
    scene.fill_rect_brush(origin, size, &Brush::Gradient(gradient));
    scene.pop_layer();
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The card every test lays out.
    const BOX: Size = Size::new(200.0, 100.0);

    /// Records the transforms, gradients, shadows and layers a paint emitted.
    #[derive(Default)]
    struct Recorder {
        transforms: Vec<Affine>,
        brushes: Vec<Brush>,
        shadows: Vec<(Point, Color)>,
        layers: Vec<f32>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn push_clip_rounded(&mut self, _origin: Point, _size: Size, _radius: f64) {
            self.clips += 1;
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn fill_rect_brush(&mut self, _origin: Point, _size: Size, brush: &Brush) {
            self.brushes.push(brush.clone());
        }
        fn draw_shadow(
            &mut self,
            origin: Point,
            _size: Size,
            _radius: f64,
            _std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, color));
        }
    }

    fn laid_out(view: &TiltCardView<()>) -> TiltCardWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(BOX));
        widget
    }

    fn card() -> TiltCardView<()> {
        tilt_card(SizedBox::<()>(Some(BOX.width), Some(BOX.height)))
    }

    fn painted(widget: &mut TiltCardWidget, ms: u64, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn send(widget: &mut TiltCardWidget, phase: PointerPhase, x: f64, y: f64) {
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
    }

    /// Settle both springs without painting.
    ///
    /// Deliberately not a sequence of `painted` calls: `PaintCtx::for_test` has
    /// no seam for seeding the hover flag, so every test paint reports "not
    /// hovered" and the paint-time sync would flatten the card being settled.
    /// Stepping the springs directly is the only way to observe a *settled*
    /// tilt from a unit test.
    fn settle(widget: &mut TiltCardWidget) {
        for step in 0..60 {
            widget.advance(FrameTime::from_nanos((100 + step * 20) * 1_000_000));
        }
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The centre reads flat and each edge reads exactly `±max/2` on its own
    /// axis — upstream's `(px − 0.5)·max` restated against the substrate's
    /// doubled offset. The vertical axis is inverted (a pointer *above* the
    /// centre lifts the top edge toward the viewer).
    #[test]
    fn the_pointer_offset_maps_onto_upstreams_own_rotation_range() {
        let mut widget = laid_out(&card());

        send(&mut widget, PointerPhase::Move, 100.0, 50.0);
        assert_eq!(widget.target_tilt(), (0.0, 0.0), "the centre is flat");

        // Right edge: a positive `ry`, half the travel.
        send(&mut widget, PointerPhase::Move, 199.0, 50.0);
        let (rx, ry) = widget.target_tilt();
        assert!(rx.abs() < 1e-9);
        assert!(
            (ry - DEFAULT_MAX_TILT / 2.0).abs() < 0.1,
            "right edge asked for {ry}"
        );

        // Top edge: a positive `rx` — upstream's `(0.5 − py)·max`.
        send(&mut widget, PointerPhase::Move, 100.0, 0.0);
        let (rx, ry) = widget.target_tilt();
        assert!(ry.abs() < 1e-9);
        assert!(
            (rx - DEFAULT_MAX_TILT / 2.0).abs() < 1e-9,
            "top edge asked for {rx}"
        );

        // The bottom edge is its mirror.
        send(&mut widget, PointerPhase::Move, 100.0, 99.0);
        let (rx, _) = widget.target_tilt();
        assert!(rx < 0.0, "the bottom edge did not invert: {rx}");

        // A wider travel scales it proportionally.
        let mut wide = laid_out(&card().max(40.0));
        send(&mut wide, PointerPhase::Move, 199.0, 50.0);
        assert!((wide.target_tilt().1 - 20.0).abs() < 0.2);
    }

    /// The affine keeps the card's centre fixed, foreshortens each axis by the
    /// cosine of its own rotation, and shears the other way — the three
    /// properties the module docs claim for the 3D approximation.
    #[test]
    fn the_tilt_transform_foreshortens_about_the_centre() {
        let mut widget = laid_out(&card());
        let centre = Point::new(BOX.width / 2.0, BOX.height / 2.0);

        // At rest it is the identity, so a card nobody is pointing at paints
        // exactly where it was laid out.
        assert_eq!(widget.tilt_transform(BOX), Affine::IDENTITY);

        send(&mut widget, PointerPhase::Move, 199.0, 50.0);
        settle(&mut widget);
        let (_, ry) = widget.tilt();
        assert!(ry > 0.0, "the spring never reached the target: {ry}");

        let transform = widget.tilt_transform(BOX);
        let moved = transform * centre;
        assert!(
            (moved - centre).hypot() < 1e-9,
            "the centre moved: {moved:?}"
        );

        // Horizontal foreshortening is exactly `cos ry`, and the vertical axis
        // is untouched by a pure `rotateY`.
        let coeffs = transform.as_coeffs();
        assert!((coeffs[0] - ry.to_radians().cos()).abs() < 1e-9);
        assert!((coeffs[3] - 1.0).abs() < 1e-9);
        // …and the shear is what stands in for the missing perspective divide.
        assert!(coeffs[1].abs() > 0.0, "no shear was applied");
    }

    /// The acceptance criterion: the tilt returns to flat once the pointer is
    /// gone, by every one of the three routes the substrate offers.
    #[test]
    fn the_tilt_resets_when_the_pointer_leaves() {
        let mut widget = laid_out(&card());
        send(&mut widget, PointerPhase::Move, 199.0, 0.0);
        settle(&mut widget);
        assert!(widget.tilt().1 > 0.0);

        // 1. A move landing outside the box.
        send(&mut widget, PointerPhase::Move, 400.0, 50.0);
        assert_eq!(widget.target_tilt(), (0.0, 0.0));
        settle(&mut widget);
        assert!(widget.tilt().1.abs() < 1e-6, "did not settle flat");

        // 2. `PaintCtx::is_hovered` — the only route that catches a pointer that
        //    left without another event.
        send(&mut widget, PointerPhase::Move, 199.0, 0.0);
        assert!(widget.is_engaged());
        painted(&mut widget, 5_000, None);
        assert!(!widget.is_engaged(), "paint did not re-sync the latch");

        // 3. An explicit release.
        send(&mut widget, PointerPhase::Move, 199.0, 0.0);
        assert!(widget.is_engaged());
        send(&mut widget, PointerPhase::Up, 199.0, 0.0);
        assert!(!widget.is_engaged());
    }

    /// A press *keeps* the tilt, which is the opposite of `marquee`'s
    /// pause-on-hover rule and the settled answer for a cursor-follow.
    #[test]
    fn a_press_keeps_the_tilt_following_the_cursor() {
        let mut widget = laid_out(&card());
        send(&mut widget, PointerPhase::Down, 199.0, 0.0);
        let (rx, ry) = widget.target_tilt();
        assert!(rx > 0.0 && ry > 0.0, "a press did not track: {rx}, {ry}");

        // A paint under the press must not flatten it, even though the
        // framework reports no hover for the whole of a press.
        painted(&mut widget, 10, None);
        assert!(widget.is_engaged(), "the paint-time sync flattened a press");

        // Dragging keeps following.
        send(&mut widget, PointerPhase::Move, 0.0, 99.0);
        let (rx, ry) = widget.target_tilt();
        assert!(rx < 0.0 && ry < 0.0, "the drag did not track: {rx}, {ry}");

        // Releasing hands the card back to the ordinary hover rule.
        send(&mut widget, PointerPhase::Up, 0.0, 99.0);
        assert_eq!(widget.target_tilt(), (0.0, 0.0));
    }

    /// The glare is on by default, paints one radial gradient at the pointer
    /// under upstream's own opacity, and can be switched off.
    #[test]
    fn the_glare_tracks_the_pointer_and_is_opt_out() {
        let mut widget = laid_out(&card());
        send(&mut widget, PointerPhase::Move, 160.0, 20.0);
        assert_eq!(widget.glare_centre(), Point::new(160.0, 20.0));

        let (recorder, _) = painted(&mut widget, 0, None);
        assert_eq!(recorder.brushes.len(), 1, "one glare per paint");
        assert!(matches!(recorder.brushes[0], Brush::Gradient(_)));
        assert_eq!(recorder.layers, vec![GLARE_OPACITY]);

        let mut plain = laid_out(&card().glare(false));
        send(&mut plain, PointerPhase::Move, 160.0, 20.0);
        let (recorder, _) = painted(&mut plain, 0, None);
        assert!(recorder.brushes.is_empty(), "glare(false) still painted");

        // At rest the glare recentres rather than freezing where the cursor
        // left — the tracker's own resting position.
        send(&mut widget, PointerPhase::Move, 900.0, 20.0);
        assert_eq!(
            widget.glare_centre(),
            Point::new(BOX.width / 2.0, BOX.height / 2.0)
        );
    }

    /// The elevation shadow is off by default (upstream paints none), and when
    /// asked for it appears only once the card is actually leaning.
    #[test]
    fn the_shadow_is_opt_in_and_scales_with_the_lean() {
        let mut plain = laid_out(&card());
        send(&mut plain, PointerPhase::Move, 199.0, 0.0);
        let (recorder, _) = painted(&mut plain, 0, None);
        assert!(recorder.shadows.is_empty(), "upstream paints no shadow");

        // A flat card casts nothing even with the affordance on.
        let mut lifted = laid_out(&card().shadow(true));
        let (recorder, _) = painted(&mut lifted, 0, None);
        assert!(recorder.shadows.is_empty(), "a flat card cast a shadow");

        // A leaning one casts a shadow displaced away from the lift.
        send(&mut lifted, PointerPhase::Move, 199.0, 0.0);
        settle(&mut lifted);
        let (recorder, _) = painted(&mut lifted, 5_000, None);
        assert_eq!(recorder.shadows.len(), 1);
        let (at, colour) = recorder.shadows[0];
        assert!(at.x < 0.0 && at.y > 0.0, "the shadow did not slide: {at:?}");
        assert!(colour.components[3] > 0.0, "the shadow was fully clear");
    }

    /// `reduce_motion` is upstream's `enabled` gate: no tilt, no glare, no
    /// tracking and no frames — the card is a plain clipped surface.
    #[test]
    fn reduce_motion_paints_a_flat_card() {
        let theme = reduced();
        let mut widget = laid_out(&card().shadow(true));
        send(&mut widget, PointerPhase::Move, 199.0, 0.0);
        assert!(widget.is_engaged());

        let (recorder, needs_frame) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        assert!(!widget.is_engaged(), "the tracker kept following");
        assert_eq!(widget.tilt(), (0.0, 0.0));
        assert!(recorder.transforms.is_empty(), "a tilt was still applied");
        assert!(recorder.brushes.is_empty(), "the glare still painted");
        assert!(recorder.shadows.is_empty());
        assert_eq!(recorder.clips, 1, "the rounded clip still applies");
    }

    /// The card is a transparent wrapper: it takes the child's size and asks for
    /// frames only while a spring is in flight.
    #[test]
    fn the_card_wraps_its_child_and_settles() {
        let mut widget = laid_out(&card());
        let (_, needs_frame) = painted(&mut widget, 0, None);
        assert!(!needs_frame, "a resting card asked for a frame");

        send(&mut widget, PointerPhase::Move, 199.0, 0.0);
        let (_, needs_frame) = painted(&mut widget, 10, None);
        assert!(needs_frame, "a retargeted spring owes a frame");

        settle(&mut widget);
        let (_, needs_frame) = painted(&mut widget, 9_000, None);
        assert!(!needs_frame, "a settled spring kept asking");
    }

    /// A zero-extent card produces no glare and no division by zero.
    #[test]
    fn a_zero_sized_card_is_inert() {
        let mut widget = laid_out(&tilt_card(SizedBox::<()>(Some(0.0), Some(0.0))));
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, Size::ZERO, FrameTime::from_nanos(0));
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        assert!(recorder.brushes.is_empty());
        assert_eq!(widget.target_tilt(), (0.0, 0.0));
    }
}
