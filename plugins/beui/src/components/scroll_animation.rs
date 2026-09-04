//! Ports beUI's `scroll-animation` component family (several upstream variants).
//!
//! **Sources:** the registry's `scroll-animation` entry names five files, beUI
//! rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01 —
//! `components/motion/smooth-scroll.tsx` (the entry's own `file`) plus its four
//! `extraFiles`, `scroll-progress.tsx`, `parallax.tsx`, `scroll-to.tsx` and
//! `scroll-reveal.tsx`. Four of the five are ported here as one module, in that
//! registry order:
//!
//! | Upstream | Here |
//! |---|---|
//! | `<ScrollProgress variant="bar" \| "circle">` | [`scroll_progress`], [`ScrollProgressVariant`] |
//! | `<Parallax speed axis>` | [`parallax`], [`ParallaxAxis`] |
//! | `<ScrollTo to offset duration>` | [`scroll_to`] |
//! | `<ScrollReveal y blur duration delay once amount>` | [`scroll_reveal`] |
//! | `<SmoothScroll>` (Lenis) | **not ported** — see below |
//!
//! # Where the scroll position comes from
//!
//! Upstream's four components all sit on a continuously readable scroll value:
//! Motion's `useScroll`, or the `useSmoothScroll()` context Lenis publishes.
//! frust has no such thing. `ScrollView::on_scroll` is the **sole** delivery
//! route for a scroll position — there is no mid-paint query and no readable
//! controller — so every component here is a *controlled* one: the app folds the
//! callback into a [`ScrollFx`](crate::motion::ScrollFx) and passes the
//! resulting `0..=1` progress in as a prop, exactly the way
//! [`crate::motion::scroll_fx`] describes.
//!
//! That also fixes what "progress" means. Upstream's `useScroll({ target })`
//! measures *one element's* crossing of the viewport; a `ScrollFx` measures the
//! **surface's** travel. For a full-page reading indicator the two agree. For a
//! parallax layer half-way down a long page they do not, and the app is the
//! thing that knows the difference — so the prop is a plain number rather than
//! a target reference this tier could not resolve anyway.
//!
//! # Not ported: `SmoothScroll` (the Lenis hijack)
//!
//! Upstream's entry file wraps the page in `ReactLenis`, which cancels native
//! wheel scrolling and re-integrates the position itself with a `lerp`, an
//! `expo-out` duration curve and a `wheelMultiplier`. **frust owns its scroll
//! physics.** `ScrollView` runs a real `ScrollPhysics` strategy (platform-parity
//! clamping/bouncing, fling simulation, overscroll), and a component tier
//! reimplementing that on top would be a second, competing physics — the exact
//! thing the framework's pluggable-physics seam exists to avoid. There is also
//! no seam for it: nothing in the widget tier can intercept a scroll gesture
//! before `ScrollView` integrates it.
//!
//! So the provider has no counterpart, and the two things it *published* are
//! served instead by [`ScrollFx`](crate::motion::ScrollFx) (offset, progress,
//! velocity) and by [`scroll_to`] (the programmatic ease). This is a
//! **non-port**, recorded for `docs/LIMITATIONS.md`, not an omission.
//!
//! # Degradation: `ScrollTo` animates an offset it cannot apply
//!
//! Upstream's `scrollTo` calls into Lenis (or `window.scrollTo`) and the page
//! moves. Verified against the assigned base: **frust's `ScrollView` exposes no
//! programmatic-scroll seam at all** — its whole surface is `on_scroll`,
//! `on_refresh_release`, `physics` and `overscroll_effect`, and while
//! `ScrollWidget::offset()` reads the position there is no setter, no
//! controller, and no view-level target prop. There is therefore nothing for a
//! component in this tier to drive.
//!
//! What [`scroll_to`] ports is the *animation*: on activation it runs upstream's
//! own Lenis easing ([`ease_scroll`]) over upstream's own default duration
//! ([`SCROLL_TO_DURATION`]) from the current offset to the requested one, and
//! **publishes each frame's value into a caller-owned signal**. An app binds
//! that signal to whatever consumes an offset. It is a real port of the curve,
//! the duration and the reduced-motion jump; it is not a port of the effect on a
//! scroll surface, and it cannot be until the framework grows a controller.
//! Recorded as a framework finding rather than worked around here.
//!
//! # The two local springs
//!
//! `scroll-progress.tsx` and `parallax.tsx` each declare the identical local
//! const — `{ stiffness: 120, damping: 30, mass: 0.6 }`, both commented *"looser
//! than the UI springs in lib/ease.ts on purpose"*. Being deliberately **not** a
//! token upstream, it is not folded onto one here either; it is
//! [`SCROLL_FOLLOW_SPRING`], the same local-number-wins call
//! [`tabs`](super::tabs) makes for its indicator.

use std::f64::consts::TAU;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CursorIcon, ErasedCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene,
    PointerPhase, Role, SemanticsCtx, Shape, View, Widget, any, build_child, erase_callback,
    rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{FrameTime, GetUntracked, RwSignal, Set, SpringDescription, Theme};
use kurbo::{Arc, Point, Size, Vec2};

use crate::motion::Ramp;
use crate::press::{SpringScalar, inside, is_activation_key, presses};
use crate::style;
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::EASE_OUT;

// ---- Shared -----------------------------------------------------------------

/// The soft follow both `scroll-progress.tsx` and `parallax.tsx` declare
/// locally — `{ stiffness: 120, damping: 30, mass: 0.6 }`, upstream's own
/// comment being *"looser than the UI springs in `lib/ease.ts` on purpose"*.
///
/// Deliberately not folded onto a [`crate::tokens::motion`] constant: upstream
/// chose *not* to make it a token, and the whole point of the number is that it
/// lags further behind than any UI spring would.
pub const SCROLL_FOLLOW_SPRING: SpringDescription = SpringDescription {
    mass: 0.6,
    stiffness: 120.0,
    damping: 30.0,
};

/// Unthemed fallback ink (beUI light `--foreground`) — `bg-foreground` /
/// `text-foreground`, what every indicator here paints with.
const FALLBACK_FOREGROUND: Color = BEUI_LIGHT.foreground;

/// The progress ring's track opacity — upstream's `opacity-15`.
pub const RING_TRACK_ALPHA: f32 = 0.15;

/// The bar's width when nothing constrains it — a reading indicator is normally
/// stretched across a container, so an unbounded one gets a plausible default
/// rather than collapsing to nothing.
const UNBOUNDED_BAR_WIDTH: f64 = 320.0;

/// Path flattening tolerance for the ring's arcs.
const ARC_TOLERANCE: f64 = style::PATH_TOLERANCE;

// ---- ScrollProgress ---------------------------------------------------------

/// The bar's default thickness in logical px — upstream's `height = 2`.
pub const DEFAULT_BAR_THICKNESS: f64 = 2.0;

/// The ring's default diameter in logical px — upstream's `size = 40`.
pub const DEFAULT_RING_DIAMETER: f64 = 40.0;

/// The ring's default stroke width in logical px — upstream's `thickness = 3`.
pub const DEFAULT_RING_THICKNESS: f64 = 3.0;

/// Which shape a [`ScrollProgressView`] draws — upstream's `variant` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollProgressVariant {
    /// `variant="bar"` (upstream's default): a horizontal fill anchored to its
    /// leading edge, `origin-left scaleX(progress)`.
    #[default]
    Bar,
    /// `variant="circle"`: a ring whose stroke sweeps clockwise from twelve
    /// o'clock.
    Ring,
}

impl ScrollProgressVariant {
    /// Both, in upstream's own declaration order.
    pub const ALL: [ScrollProgressVariant; 2] =
        [ScrollProgressVariant::Bar, ScrollProgressVariant::Ring];
}

/// Which edge of its own box a [`ScrollProgressVariant::Bar`] sits against —
/// upstream's `position: "top" | "bottom"`.
///
/// Upstream's `fixed` prop has no counterpart: it chooses between CSS `fixed`
/// and `absolute` positioning, and where a widget sits in the tree is the app's
/// layout decision here, not a prop this component can honour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollProgressPosition {
    /// Against the top edge (upstream's default).
    #[default]
    Top,
    /// Against the bottom edge.
    Bottom,
}

impl ScrollProgressPosition {
    /// Both, in upstream's own order.
    pub const ALL: [ScrollProgressPosition; 2] =
        [ScrollProgressPosition::Top, ScrollProgressPosition::Bottom];
}

/// A declarative beUI scroll-progress indicator. See the [module docs](self) —
/// `progress` is a `0..=1` fraction the app folds out of `ScrollView::on_scroll`.
///
/// # Example
///
/// ```
/// use frust_beui::components::scroll_animation::{ScrollProgressVariant, scroll_progress};
///
/// let ring = scroll_progress::<()>(0.42).variant(ScrollProgressVariant::Ring);
/// ```
pub struct ScrollProgressView<State: 'static> {
    progress: f64,
    variant: ScrollProgressVariant,
    position: ScrollProgressPosition,
    spring: bool,
    thickness: f64,
    diameter: f64,
    _state: std::marker::PhantomData<State>,
}

/// A reading indicator at `progress` (`0..=1`), as a [`ScrollProgressVariant::Bar`]
/// with upstream's own defaults.
pub fn scroll_progress<State: 'static>(progress: f64) -> ScrollProgressView<State> {
    ScrollProgressView {
        progress: progress.clamp(0.0, 1.0),
        variant: ScrollProgressVariant::default(),
        position: ScrollProgressPosition::default(),
        spring: true,
        thickness: DEFAULT_BAR_THICKNESS,
        diameter: DEFAULT_RING_DIAMETER,
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> ScrollProgressView<State> {
    /// Which shape to draw (`variant`, default [`ScrollProgressVariant::Bar`]).
    pub fn variant(mut self, variant: ScrollProgressVariant) -> Self {
        self.variant = variant;
        if variant == ScrollProgressVariant::Ring && self.thickness == DEFAULT_BAR_THICKNESS {
            // The two variants carry different upstream defaults for the same
            // prop name (`height = 2` against `thickness = 3`), so adopting the
            // ring's is what "not setting it" means.
            self.thickness = DEFAULT_RING_THICKNESS;
        }
        self
    }

    /// Which edge a bar sits against (`position`, default
    /// [`ScrollProgressPosition::Top`]). Ignored by the ring.
    pub fn position(mut self, position: ScrollProgressPosition) -> Self {
        self.position = position;
        self
    }

    /// Spring-smooth the value (`spring`, default `true`). Automatically off
    /// under the theme's `reduce_motion`, upstream's own rule.
    pub fn spring(mut self, spring: bool) -> Self {
        self.spring = spring;
        self
    }

    /// The bar's thickness or the ring's stroke width, in logical px
    /// (upstream's `height = 2` and `thickness = 3` respectively).
    pub fn thickness(mut self, thickness: f64) -> Self {
        self.thickness = thickness.max(0.0);
        self
    }

    /// The ring's diameter in logical px (`size`, default
    /// [`DEFAULT_RING_DIAMETER`]). Ignored by the bar.
    pub fn diameter(mut self, diameter: f64) -> Self {
        self.diameter = diameter.max(0.0);
        self
    }
}

/// The retained widget for a [`ScrollProgressView`].
pub struct ScrollProgressWidget {
    variant: ScrollProgressVariant,
    position: ScrollProgressPosition,
    spring: bool,
    thickness: f64,
    diameter: f64,
    /// The app-reported progress — the value the smoothing follows.
    progress: f64,
    /// The displayed value, trailing `progress` on [`SCROLL_FOLLOW_SPRING`].
    shown: SpringScalar,
}

impl ScrollProgressWidget {
    /// The progress last reported by the app, `0..=1`.
    pub fn progress(&self) -> f64 {
        self.progress
    }

    /// The value actually drawn: the reported progress, or the spring's trailing
    /// read of it, clamped — a bar can no more overshoot past full than an
    /// opacity can.
    pub fn shown(&self) -> f64 {
        self.shown.value().clamp(0.0, 1.0)
    }
}

impl<State: 'static> View<State> for ScrollProgressView<State> {
    type Element = ScrollProgressWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ScrollProgressWidget {
        ScrollProgressWidget {
            variant: self.variant,
            position: self.position,
            spring: self.spring,
            thickness: self.thickness,
            diameter: self.diameter,
            progress: self.progress,
            // A freshly-built indicator rests on the value it was given rather
            // than sweeping up to it from zero.
            shown: SpringScalar::new(self.progress, Ramp::spring(SCROLL_FOLLOW_SPRING)),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollProgressWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.progress != self.progress {
            element.progress = self.progress;
            if element.spring {
                element.shown.set_target(self.progress);
            } else {
                element.shown.jump_to(self.progress);
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.position != self.position {
            element.position = self.position;
            flags |= ChangeFlags::PAINT;
        }
        if element.spring != self.spring {
            element.spring = self.spring;
            if !self.spring {
                element.shown.jump_to(element.progress);
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.thickness != self.thickness || element.diameter != self.diameter {
            element.thickness = self.thickness;
            element.diameter = self.diameter;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for ScrollProgressWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let natural = match self.variant {
            ScrollProgressVariant::Bar => {
                let width = if bc.max().width.is_finite() {
                    bc.max().width
                } else {
                    UNBOUNDED_BAR_WIDTH
                };
                Size::new(width, self.thickness)
            }
            ScrollProgressVariant::Ring => Size::new(self.diameter, self.diameter),
        };
        bc.constrain(natural)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink = theme.map_or(FALLBACK_FOREGROUND, |t| t.scheme().on_surface);
        let origin = ctx.origin();
        let size = ctx.size();

        // Upstream's `spring && !reduce`: the flag alone is not enough, a
        // reduced-motion reader gets the raw value.
        if reduce {
            self.shown.jump_to(self.progress);
        }
        self.shown.advance(ctx.frame_time());
        let value = self.shown();

        match self.variant {
            ScrollProgressVariant::Bar => {
                let thickness = self.thickness.min(size.height);
                let top = match self.position {
                    ScrollProgressPosition::Top => origin.y,
                    ScrollProgressPosition::Bottom => origin.y + size.height - thickness,
                };
                // `origin-left scaleX(value)` — a fill anchored to the leading
                // edge, not a centred scale.
                let width = size.width * value;
                if width > 0.0 && thickness > 0.0 {
                    scene.fill_rect(Point::new(origin.x, top), Size::new(width, thickness), ink);
                }
            }
            ScrollProgressVariant::Ring => {
                let extent = size.width.min(size.height);
                let radius = ((extent - self.thickness) / 2.0).max(0.0);
                if radius <= 0.0 || self.thickness <= 0.0 {
                    return;
                }
                let centre = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
                let track = Arc::new(centre, Vec2::new(radius, radius), 0.0, TAU, 0.0);
                scene.stroke_path(
                    Point::ORIGIN,
                    &Shape::to_path(&track, ARC_TOLERANCE),
                    self.thickness,
                    &Brush::Solid(style::scale_alpha(ink, RING_TRACK_ALPHA)),
                );
                // `transform="rotate(-90 …)"` with a dash offset of
                // `circumference · (1 − value)`: a sweep from twelve o'clock.
                let sweep = TAU * value;
                if sweep > 0.0 {
                    let head = Arc::new(centre, Vec2::new(radius, radius), -TAU / 4.0, sweep, 0.0);
                    scene.stroke_path(
                        Point::ORIGIN,
                        &Shape::to_path(&head, ARC_TOLERANCE),
                        self.thickness,
                        &Brush::Solid(ink),
                    );
                }
            }
        }

        if self.shown.is_animating() {
            ctx.request_frame();
        }
    }

    // No `semantics` arm: both variants carry `aria-hidden` upstream, because
    // the indicator only restates a scroll position assistive tech already
    // conveys. The trait's empty default is exactly that.
}

// ---- Parallax ---------------------------------------------------------------

/// Upstream's `speed = 0.3` — the drift as a fraction of the element's travel.
pub const DEFAULT_PARALLAX_SPEED: f64 = 0.3;

/// The px a `speed` of `1.0` drifts across the whole progress range, either way
/// from centre — upstream's `travel = speed * 100`.
pub const PARALLAX_TRAVEL_PER_SPEED: f64 = 100.0;

/// Which axis a [`ParallaxView`] drifts along — upstream's `axis` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParallaxAxis {
    /// `axis="y"` (upstream's default): vertical drift.
    #[default]
    Y,
    /// `axis="x"`: horizontal drift.
    X,
}

impl ParallaxAxis {
    /// Both, in upstream's own order.
    pub const ALL: [ParallaxAxis; 2] = [ParallaxAxis::Y, ParallaxAxis::X];
}

/// A declarative beUI parallax layer: a child offset against the scroll. See the
/// [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::scroll_animation::{ParallaxView, parallax};
///
/// let layer: ParallaxView<()> = parallax(text("backdrop"), 0.25).speed(-0.4);
/// ```
pub struct ParallaxView<State: 'static> {
    child: AnyView<State>,
    progress: f64,
    speed: f64,
    axis: ParallaxAxis,
    spring: bool,
}

/// Drift `child` against a scroll surface at `progress` (`0..=1`), with
/// upstream's own defaults.
pub fn parallax<State: 'static, V: View<State>>(child: V, progress: f64) -> ParallaxView<State> {
    ParallaxView {
        child: any(child),
        progress: progress.clamp(0.0, 1.0),
        speed: DEFAULT_PARALLAX_SPEED,
        axis: ParallaxAxis::default(),
        spring: true,
    }
}

impl<State: 'static> ParallaxView<State> {
    /// The drift rate (`speed`, default [`DEFAULT_PARALLAX_SPEED`]). Positive
    /// moves with the scroll (foreground), negative against it (background);
    /// upstream reckons `~0.1–0.5` reads best.
    pub fn speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    /// Which axis to drift along (`axis`, default [`ParallaxAxis::Y`]).
    pub fn axis(mut self, axis: ParallaxAxis) -> Self {
        self.axis = axis;
        self
    }

    /// Spring-smooth the drift (`spring`, default `true`). Irrelevant under
    /// `reduce_motion`, which removes the drift outright.
    pub fn spring(mut self, spring: bool) -> Self {
        self.spring = spring;
        self
    }
}

/// The retained widget for a [`ParallaxView`].
pub struct ParallaxWidget {
    child: ChildPod,
    progress: f64,
    speed: f64,
    axis: ParallaxAxis,
    spring: bool,
    /// The displayed drift in px, trailing the target on
    /// [`SCROLL_FOLLOW_SPRING`].
    shown: SpringScalar,
}

/// The drift `speed` asks for at `progress`: upstream's
/// `useTransform(scrollYProgress, [0, 1], [travel, -travel])`, so a layer starts
/// displaced *toward* the scroll and ends displaced against it, passing through
/// zero at the halfway point.
pub fn parallax_drift(speed: f64, progress: f64) -> f64 {
    let travel = speed * PARALLAX_TRAVEL_PER_SPEED;
    travel * (1.0 - 2.0 * progress.clamp(0.0, 1.0))
}

impl ParallaxWidget {
    /// The drift the current progress asks for, in px.
    pub fn target_drift(&self) -> f64 {
        parallax_drift(self.speed, self.progress)
    }

    /// The drift actually applied, in px — the sprung read when smoothing is on.
    pub fn drift(&self) -> f64 {
        self.shown.value()
    }

    /// The drift as a translation on this layer's own axis.
    fn translation(&self) -> Vec2 {
        match self.axis {
            ParallaxAxis::Y => Vec2::new(0.0, self.drift()),
            ParallaxAxis::X => Vec2::new(self.drift(), 0.0),
        }
    }
}

impl<State: 'static> View<State> for ParallaxView<State> {
    type Element = ParallaxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ParallaxWidget {
        let drift = parallax_drift(self.speed, self.progress);
        ParallaxWidget {
            child: build_child(&self.child, ctx),
            progress: self.progress,
            speed: self.speed,
            axis: self.axis,
            spring: self.spring,
            // Built already displaced: a layer entering the tree mid-page must
            // not slide in from its zero position.
            shown: SpringScalar::new(drift, Ramp::spring(SCROLL_FOLLOW_SPRING)),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ParallaxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.axis != self.axis {
            element.axis = self.axis;
            flags |= ChangeFlags::PAINT;
        }
        if element.spring != self.spring {
            element.spring = self.spring;
            flags |= ChangeFlags::PAINT;
        }
        if prev.progress != self.progress || element.speed != self.speed {
            element.progress = self.progress;
            element.speed = self.speed;
            let drift = element.target_drift();
            if element.spring {
                element.shown.set_target(drift);
            } else {
                element.shown.jump_to(drift);
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ParallaxWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ParallaxWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A paint-time offset, never a layout one: a drifting layer must not
        // re-flow the page it is drifting inside.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        if reduce {
            // Upstream's `style = reduce ? {} : …`: no drift at all, not merely
            // an unsmoothed one.
            self.shown.jump_to(0.0);
            self.child.paint_child(ctx, scene);
            return;
        }
        self.shown.advance(ctx.frame_time());
        scene.push_transform(Affine::translate(self.translation()));
        self.child.paint_child(ctx, scene);
        scene.pop_transform();
        if self.shown.is_animating() {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The drift is a paint transform, so a child's laid-out box is not where
        // it is drawn — but routing by the laid-out box is still the closest
        // honest answer, and it is what keeps a child's pods live.
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

// ---- ScrollReveal -----------------------------------------------------------

/// Upstream's `y = 16` — how far the child slides up into place.
pub const DEFAULT_REVEAL_SLIDE: f64 = 16.0;

/// Upstream's `duration = 0.6`.
pub const DEFAULT_REVEAL_DURATION: Duration = Duration::from_millis(600);

/// Upstream's `amount = 0.3` — the progress a reveal triggers at. The same
/// number [`crate::motion::scroll_fx::DEFAULT_REVEAL_THRESHOLD`] carries, for
/// the same reason.
pub const DEFAULT_REVEAL_THRESHOLD: f64 = crate::motion::scroll_fx::DEFAULT_REVEAL_THRESHOLD;

/// A declarative beUI scroll reveal: a child that fades and slides in once the
/// surface has scrolled far enough. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::scroll_animation::{ScrollRevealView, scroll_reveal};
///
/// let section: ScrollRevealView<()> = scroll_reveal(text("hello"), 0.4)
///     .once(false)
///     .slide(24.0);
/// ```
pub struct ScrollRevealView<State: 'static> {
    child: AnyView<State>,
    progress: f64,
    threshold: f64,
    once: bool,
    slide: f64,
    duration: Duration,
    delay: Duration,
}

/// Reveal `child` once a surface at `progress` (`0..=1`) crosses
/// [`DEFAULT_REVEAL_THRESHOLD`], with upstream's own defaults.
pub fn scroll_reveal<State: 'static, V: View<State>>(
    child: V,
    progress: f64,
) -> ScrollRevealView<State> {
    ScrollRevealView {
        child: any(child),
        progress: progress.clamp(0.0, 1.0),
        threshold: DEFAULT_REVEAL_THRESHOLD,
        once: true,
        slide: DEFAULT_REVEAL_SLIDE,
        duration: DEFAULT_REVEAL_DURATION,
        delay: Duration::ZERO,
    }
}

impl<State: 'static> ScrollRevealView<State> {
    /// The progress the reveal triggers at (`amount`, default
    /// [`DEFAULT_REVEAL_THRESHOLD`]), clamped into `0..=1`.
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Reveal only once (`once`, upstream's `true` default), or every time the
    /// threshold is crossed.
    ///
    /// The two differ only on the way *back*: a `once` reveal that has fired
    /// stays revealed when the surface scrolls back above the threshold, so its
    /// content does not flicker on a scroll-up.
    pub fn once(mut self, once: bool) -> Self {
        self.once = once;
        self
    }

    /// How far the child slides up into place (`y`, default
    /// [`DEFAULT_REVEAL_SLIDE`]).
    pub fn slide(mut self, slide: f64) -> Self {
        self.slide = slide;
        self
    }

    /// How long the reveal takes (`duration`, default
    /// [`DEFAULT_REVEAL_DURATION`]).
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// How long to wait after the threshold is crossed before starting
    /// (`delay`, default none).
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// The retained widget for a [`ScrollRevealView`].
pub struct ScrollRevealWidget {
    child: ChildPod,
    progress: f64,
    threshold: f64,
    once: bool,
    slide: f64,
    duration: Duration,
    delay: Duration,
    /// Whether the threshold is currently considered crossed — latched under
    /// `once`, live otherwise.
    revealed: bool,
    /// The reveal's `0..=1` position, on an [`EASE_OUT`] ramp.
    shown: SpringScalar,
    /// When the pending flip was first seen, so [`ScrollRevealView::delay`] is
    /// measured from a real frame rather than from a rebuild.
    armed_at: Option<FrameTime>,
}

impl ScrollRevealWidget {
    /// Whether the threshold is currently considered crossed.
    pub fn revealed(&self) -> bool {
        self.revealed
    }

    /// The reveal's displayed position: `0.0` fully hidden, `1.0` fully in
    /// place.
    pub fn shown(&self) -> f64 {
        self.shown.value().clamp(0.0, 1.0)
    }

    /// Fold `progress` into the reveal latch, returning whether it changed.
    ///
    /// Upstream's `once` is `useInView`'s own: the latch never clears. With
    /// `once: false` the reveal is live in both directions, so scrolling back
    /// above the threshold hides the child again.
    fn observe(&mut self, progress: f64) -> bool {
        let next = if progress >= self.threshold {
            true
        } else if self.once {
            self.revealed
        } else {
            false
        };
        if next == self.revealed {
            return false;
        }
        self.revealed = next;
        self.armed_at = None;
        true
    }
}

impl<State: 'static> View<State> for ScrollRevealView<State> {
    type Element = ScrollRevealWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollRevealWidget {
        // A surface already past the threshold when the child mounts is
        // revealed, not animated into place — upstream's `useInView` reports the
        // same on its first measurement.
        let revealed = self.progress >= self.threshold;
        ScrollRevealWidget {
            child: build_child(&self.child, ctx),
            progress: self.progress,
            threshold: self.threshold,
            once: self.once,
            slide: self.slide,
            duration: self.duration,
            delay: self.delay,
            revealed,
            shown: SpringScalar::new(
                if revealed { 1.0 } else { 0.0 },
                Ramp::eased(self.duration, EASE_OUT),
            ),
            armed_at: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollRevealWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if element.threshold != self.threshold || element.once != self.once {
            element.threshold = self.threshold;
            element.once = self.once;
            flags |= ChangeFlags::PAINT;
        }
        if element.slide != self.slide {
            element.slide = self.slide;
            flags |= ChangeFlags::PAINT;
        }
        element.delay = self.delay;
        if element.duration != self.duration {
            element.duration = self.duration;
            // `SpringScalar` fixes its ramp at construction, so a changed
            // duration is a fresh scalar resting on what is currently on screen
            // — the same value-continuous restart a retarget performs.
            element.shown =
                SpringScalar::new(element.shown.value(), Ramp::eased(self.duration, EASE_OUT));
            flags |= ChangeFlags::PAINT;
        }
        if prev.progress != self.progress {
            element.progress = self.progress;
            flags |= ChangeFlags::PAINT;
        }
        if element.observe(element.progress) {
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ScrollRevealWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ScrollRevealWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The slide is a paint offset: a revealing section must not re-flow the
        // page under everything below it.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let want = if self.revealed { 1.0 } else { 0.0 };
        let mut owes_frame = false;
        if self.shown.target() != want {
            if reduce {
                self.shown.jump_to(want);
            } else {
                // The delay is measured from the first frame that saw the flip,
                // not from the rebuild that staged it — a rebuild carries no
                // clock.
                let armed = *self.armed_at.get_or_insert(now);
                if now.saturating_sub(armed) >= self.delay {
                    self.shown.set_target(want);
                    self.armed_at = None;
                } else {
                    owes_frame = true;
                }
            }
        }
        self.shown.advance(now);
        owes_frame |= self.shown.is_animating();
        let shown = self.shown();

        if shown >= 1.0 {
            self.child.paint_child(ctx, scene);
        } else {
            // Upstream's reduced-motion `hidden` is `{ opacity: 0 }` alone; the
            // full one adds `y` (and a blur this tier has no primitive for —
            // see the module docs).
            let slide = if reduce {
                0.0
            } else {
                self.slide * (1.0 - shown)
            };
            scene.push_layer(origin, size, shown as f32);
            scene.push_transform(Affine::translate(Vec2::new(0.0, slide)));
            self.child.paint_child(ctx, scene);
            scene.pop_transform();
            scene.pop_layer();
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

// ---- ScrollTo ---------------------------------------------------------------

/// Upstream's Lenis default `duration = 1.2` seconds.
pub const SCROLL_TO_DURATION: Duration = Duration::from_millis(1200);

/// Lenis' own expo-out easing, `t => min(1, 1.001 − 2^(−10·t))` — upstream's
/// `EASE_SCROLL`, kept as a function rather than folded onto a
/// [`crate::tokens::motion`] curve because it is not a cubic bezier and no
/// bezier reproduces it.
///
/// Upstream's own comment says as much: the token table holds bezier control
/// points for the motion library, while Lenis needs a `(t) => number`.
pub fn ease_scroll(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    (1.001 - 2f64.powf(-10.0 * t)).min(1.0)
}

/// A view-held activation callback (erased on build).
type OnScrollTo<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI scroll-to control: a button that eases a caller-owned
/// offset signal toward a target.
///
/// **Read the [module docs](self) first** — this animates an offset, it does not
/// move a scroll surface, because the framework publishes nothing to move.
///
/// # Example
///
/// ```
/// use frust::{RwSignal, text};
/// use frust_beui::components::scroll_animation::{ScrollToView, scroll_to};
///
/// let offset = RwSignal::new(0.0);
/// let button: ScrollToView<()> = scroll_to(text("back to top"), 0.0, offset);
/// ```
pub struct ScrollToView<State: 'static> {
    child: AnyView<State>,
    to: f64,
    extra_offset: f64,
    duration: Duration,
    disabled: bool,
    offset: RwSignal<f64>,
    on_activate: Option<OnScrollTo<State>>,
}

/// A control that eases `offset` toward `to` on activation.
///
/// `offset` is read for the starting position and written on every frame of the
/// ease — the same publish-into-signals shape [`crate::motion::ScrollFxSignals`]
/// uses, and with the same cost: every tracked reader wakes per frame while the
/// ease runs.
pub fn scroll_to<State: 'static, V: View<State>>(
    child: V,
    to: f64,
    offset: RwSignal<f64>,
) -> ScrollToView<State> {
    ScrollToView {
        child: any(child),
        to,
        extra_offset: 0.0,
        duration: SCROLL_TO_DURATION,
        disabled: false,
        offset,
        on_activate: None,
    }
}

impl<State: 'static> ScrollToView<State> {
    /// Extra px added to the target (`offset` — upstream's "clear a sticky
    /// header" prop).
    pub fn extra_offset(mut self, extra_offset: f64) -> Self {
        self.extra_offset = extra_offset;
        self
    }

    /// How long the ease takes (`duration`, default [`SCROLL_TO_DURATION`]).
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Disable the control: inert, and painted by whatever the child chooses.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Also run `callback` when the control is activated, before the ease
    /// starts — the seam an app closes an overlay or records analytics through.
    pub fn on_activate<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_activate = Some(Rc::new(callback));
        self
    }
}

/// A running programmatic ease.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollRun {
    from: f64,
    to: f64,
    /// The frame it started on; `None` until `paint` stamps one.
    start: Option<FrameTime>,
}

/// The retained widget for a [`ScrollToView`].
pub struct ScrollToWidget {
    child: ChildPod,
    to: f64,
    extra_offset: f64,
    duration: Duration,
    disabled: bool,
    offset: RwSignal<f64>,
    run: Option<ScrollRun>,
    /// Whether a press began inside this control, so a release outside it does
    /// not activate.
    armed: bool,
    on_activate: Option<ErasedCallback>,
}

impl ScrollToWidget {
    /// The offset this control eases toward — `to` plus the extra offset.
    pub fn target(&self) -> f64 {
        self.to + self.extra_offset
    }

    /// Whether an ease is currently running.
    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// Begin the ease, or jump straight to the target under `reduce_motion`.
    ///
    /// The starting position is read from the signal rather than remembered, so
    /// a control activated while something else is moving the offset eases from
    /// where the offset actually is.
    fn start(&mut self, reduce: bool) {
        let target = self.target();
        if reduce {
            self.run = None;
            self.offset.set(target);
            return;
        }
        self.run = Some(ScrollRun {
            from: self.offset.get_untracked(),
            to: target,
            start: None,
        });
    }

    /// Advance a running ease to `now`, publishing the value. Returns whether
    /// one is still running.
    fn advance(&mut self, now: FrameTime) -> bool {
        let Some(mut run) = self.run else {
            return false;
        };
        let start = *run.start.get_or_insert(now);
        self.run = Some(run);
        if self.duration.is_zero() || run.from == run.to {
            self.offset.set(run.to);
            self.run = None;
            return false;
        }
        let elapsed = now.saturating_sub(start);
        if elapsed >= self.duration {
            self.offset.set(run.to);
            self.run = None;
            return false;
        }
        let t = elapsed.as_secs_f64() / self.duration.as_secs_f64();
        self.offset
            .set(run.from + (run.to - run.from) * ease_scroll(t));
        true
    }
}

impl<State: 'static> View<State> for ScrollToView<State> {
    type Element = ScrollToWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollToWidget {
        ScrollToWidget {
            child: build_child(&self.child, ctx),
            to: self.to,
            extra_offset: self.extra_offset,
            duration: self.duration,
            disabled: self.disabled,
            offset: self.offset,
            run: None,
            armed: false,
            on_activate: self.on_activate.as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollToWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_activate = self.on_activate.as_ref().map(erase_callback);
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        element.to = self.to;
        element.extra_offset = self.extra_offset;
        element.duration = self.duration;
        element.offset = self.offset;
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-ease abandons it rather than finishing
                // a scroll nobody can now stop.
                element.run = None;
                element.armed = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ScrollToWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ScrollToWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        self.child.paint_child(ctx, scene);
        if reduce {
            // An event pass sees no theme, so an activation always stages a run
            // and the first paint is where reduced motion collapses it — the
            // jump upstream's `scrollTo` performs under the same flag.
            if let Some(run) = self.run.take() {
                self.offset.set(run.to);
            }
            return;
        }
        if self.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.child.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if self.disabled {
            return EventResult::Ignored;
        }
        // No theme reaches an event pass, so the reduced-motion jump is decided
        // from the same read every other consumer takes — at paint. An
        // activation therefore always stages a run; a reduced-motion paint lands
        // it on the first frame.
        let size = ctx.size();
        match event {
            InputEvent::Key(key) if is_activation_key(key) => {
                self.start(false);
                self.armed = false;
                if let Some(callback) = &mut self.on_activate {
                    callback(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, size) {
                        return EventResult::Ignored;
                    }
                    self.armed = true;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if inside(p.position, size) {
                        ctx.claim_hover();
                        ctx.set_cursor(CursorIcon::Pointer);
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    if !self.armed {
                        return EventResult::Ignored;
                    }
                    self.armed = false;
                    // A release outside the control is a cancel, the same rule
                    // every button in the catalog follows.
                    if !inside(p.position, size) {
                        return EventResult::Handled;
                    }
                    self.start(false);
                    if let Some(callback) = &mut self.on_activate {
                        callback(ctx);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.armed = false;
                    EventResult::Ignored
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Button,
            |node| {
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.child.semantics_child(ctx),
        );
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
    };
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays its subject into.
    const BOX: Size = Size::new(200.0, 60.0);

    /// Records the fills, strokes, transforms and layer alphas a paint emitted.
    #[derive(Default)]
    struct Recorder {
        fills: Vec<(Point, Size, Color)>,
        strokes: Vec<(f64, Brush)>,
        transforms: Vec<Affine>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.fills.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn stroke_path(&mut self, _origin: Point, _path: &BezPath, width: f64, brush: &Brush) {
            self.strokes.push((width, brush.clone()));
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    fn build<V: View<()>>(view: &V) -> V::Element {
        let mut next_id = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn lay_out<W: Widget>(widget: &mut W, bc: BoxConstraints) -> Size {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &bc)
    }

    fn painted<W: Widget>(
        widget: &mut W,
        size: Size,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn rebuild<V: View<()>>(prev: &V, next: &V, element: &mut V::Element) {
        let mut next_id = 1u64;
        View::<()>::rebuild(next, prev, element, &mut BuildCtx::new(&mut next_id));
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    fn child() -> impl View<()> {
        SizedBox::<()>(Some(BOX.width), Some(BOX.height))
    }

    // ---- ScrollProgress ----

    /// The bar fills from its leading edge in proportion to progress, sits
    /// against whichever edge it was asked for, and never overshoots.
    #[test]
    fn the_progress_bar_fills_from_its_leading_edge() {
        let view = scroll_progress::<()>(0.0).spring(false).thickness(4.0);
        let mut widget = build(&view);
        let size = lay_out(&mut widget, BoxConstraints::loose(BOX));
        assert_eq!(
            size,
            Size::new(BOX.width, 4.0),
            "a bar is as thick as it was asked to be, not as tall as its room"
        );

        rebuild(
            &view,
            &scroll_progress::<()>(0.25).spring(false).thickness(4.0),
            &mut widget,
        );
        let (recorder, _) = painted(&mut widget, size, 0, None);
        let (at, filled, _) = recorder.fills[0];
        assert_eq!(at, Point::ORIGIN, "the fill is anchored left, not centred");
        assert_eq!(filled, Size::new(BOX.width * 0.25, 4.0));

        // A bottom-positioned bar is pushed to the far edge of its own box.
        let bottom = scroll_progress::<()>(1.0)
            .spring(false)
            .thickness(4.0)
            .position(ScrollProgressPosition::Bottom);
        let mut widget = build(&bottom);
        let (recorder, _) = painted(&mut widget, BOX, 0, None);
        let (at, filled, _) = recorder.fills[0];
        assert_eq!(at, Point::new(0.0, BOX.height - 4.0));
        assert_eq!(filled.width, BOX.width, "full progress fills the box");

        // Out-of-range input is clamped rather than painted past the end.
        let over = scroll_progress::<()>(4.0).spring(false);
        let widget = build(&over);
        assert_eq!(widget.progress(), 1.0);
    }

    /// Spring smoothing trails the reported value and settles onto it; turning
    /// it off (or reducing motion) reads the raw number.
    #[test]
    fn the_indicator_springs_toward_the_reported_progress() {
        let start = scroll_progress::<()>(0.0);
        let mut widget = build(&start);
        rebuild(&start, &scroll_progress::<()>(1.0), &mut widget);

        // Mid-flight it is behind the target, and owes a frame.
        painted(&mut widget, BOX, 0, None);
        let (_, needs_frame) = painted(&mut widget, BOX, 60, None);
        assert!(needs_frame, "a live spring owes a frame");
        assert!(
            widget.shown() < 1.0,
            "the spring did not lag: {}",
            widget.shown()
        );

        for step in 0..60 {
            painted(&mut widget, BOX, 100 + step * 30, None);
        }
        assert!((widget.shown() - 1.0).abs() < 1e-9, "never settled");

        // `spring(false)` is instantaneous.
        let plain = scroll_progress::<()>(0.0).spring(false);
        let mut widget = build(&plain);
        rebuild(
            &plain,
            &scroll_progress::<()>(0.8).spring(false),
            &mut widget,
        );
        assert!((widget.shown() - 0.8).abs() < 1e-9);

        // …and so is `reduce_motion`, whatever the flag says.
        let theme = reduced();
        let sprung = scroll_progress::<()>(0.0);
        let mut widget = build(&sprung);
        rebuild(&sprung, &scroll_progress::<()>(0.6), &mut widget);
        let (_, needs_frame) = painted(&mut widget, BOX, 0, Some(&theme));
        assert!(!needs_frame);
        assert!((widget.shown() - 0.6).abs() < 1e-9);
    }

    /// The ring is square, strokes a full track plus a swept head, and drops the
    /// head entirely at zero.
    #[test]
    fn the_progress_ring_strokes_a_track_and_a_sweep() {
        let view = scroll_progress::<()>(0.5)
            .variant(ScrollProgressVariant::Ring)
            .spring(false);
        let mut widget = build(&view);
        let size = lay_out(&mut widget, BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(
            size,
            Size::new(DEFAULT_RING_DIAMETER, DEFAULT_RING_DIAMETER),
            "the ring is square at its own diameter"
        );
        // Choosing the ring adopts the ring's own default stroke width.
        assert_eq!(widget.thickness, DEFAULT_RING_THICKNESS);

        let (recorder, _) = painted(&mut widget, size, 0, None);
        assert_eq!(recorder.strokes.len(), 2, "a track and a head");
        assert_eq!(recorder.strokes[0].0, DEFAULT_RING_THICKNESS);

        let empty = scroll_progress::<()>(0.0)
            .variant(ScrollProgressVariant::Ring)
            .spring(false);
        let mut widget = build(&empty);
        let (recorder, _) = painted(&mut widget, size, 0, None);
        assert_eq!(recorder.strokes.len(), 1, "an empty ring is track only");

        assert_eq!(ScrollProgressVariant::ALL.len(), 2);
        assert_eq!(ScrollProgressPosition::ALL.len(), 2);
    }

    // ---- Parallax ----

    /// The drift runs from `+speed·100` to `−speed·100` across the progress
    /// range, passing through zero at the halfway point — upstream's own
    /// symmetric mapping, and negative speeds mirror it.
    #[test]
    fn the_parallax_drift_is_symmetric_about_the_halfway_point() {
        assert_eq!(parallax_drift(0.3, 0.0), 30.0);
        assert_eq!(parallax_drift(0.3, 0.5), 0.0);
        assert_eq!(parallax_drift(0.3, 1.0), -30.0);
        assert_eq!(parallax_drift(-0.3, 0.0), -30.0);
        // Out-of-range progress is clamped, not extrapolated.
        assert_eq!(parallax_drift(0.3, 2.0), -30.0);

        for axis in ParallaxAxis::ALL {
            let view = parallax::<(), _>(child(), 0.0).axis(axis).spring(false);
            let mut widget = build(&view);
            lay_out(&mut widget, BoxConstraints::tight(BOX));
            let (recorder, _) = painted(&mut widget, BOX, 0, None);
            let translation = recorder.transforms[0].as_coeffs();
            match axis {
                ParallaxAxis::Y => {
                    assert_eq!((translation[4], translation[5]), (0.0, 30.0));
                }
                ParallaxAxis::X => {
                    assert_eq!((translation[4], translation[5]), (30.0, 0.0));
                }
            }
        }
    }

    /// `reduce_motion` removes the drift outright rather than merely unsmoothing
    /// it — upstream's `style = reduce ? {} : …`.
    #[test]
    fn reduce_motion_removes_the_parallax_drift() {
        let theme = reduced();
        let view = parallax::<(), _>(child(), 0.0).spring(false);
        let mut widget = build(&view);
        lay_out(&mut widget, BoxConstraints::tight(BOX));
        let (recorder, needs_frame) = painted(&mut widget, BOX, 0, Some(&theme));
        assert!(!needs_frame);
        assert!(recorder.transforms.is_empty(), "a drift was still applied");
        assert_eq!(widget.drift(), 0.0);
    }

    // ---- ScrollReveal ----

    /// The threshold contract: crossing it reveals, and `once` decides whether
    /// scrolling back hides again.
    #[test]
    fn the_reveal_threshold_latches_only_under_once() {
        let hidden = scroll_reveal::<(), _>(child(), 0.0);
        let mut widget = build(&hidden);
        assert!(!widget.revealed(), "below the threshold at build");

        // Just short of the default 0.3 is still hidden.
        rebuild(&hidden, &scroll_reveal::<(), _>(child(), 0.29), &mut widget);
        assert!(!widget.revealed());

        rebuild(&hidden, &scroll_reveal::<(), _>(child(), 0.3), &mut widget);
        assert!(widget.revealed(), "the threshold is inclusive");

        // `once` (the default) latches: scrolling back does not hide it.
        rebuild(&hidden, &scroll_reveal::<(), _>(child(), 0.0), &mut widget);
        assert!(widget.revealed(), "a once reveal un-revealed");

        // `once(false)` is live in both directions.
        let every = scroll_reveal::<(), _>(child(), 0.5).once(false);
        let mut widget = build(&every);
        assert!(widget.revealed());
        rebuild(
            &every,
            &scroll_reveal::<(), _>(child(), 0.1).once(false),
            &mut widget,
        );
        assert!(!widget.revealed(), "an every-cross reveal stayed latched");

        // A custom threshold moves the trigger.
        let late = scroll_reveal::<(), _>(child(), 0.5).threshold(0.9);
        let mut widget = build(&late);
        assert!(!widget.revealed());
        rebuild(
            &late,
            &scroll_reveal::<(), _>(child(), 0.95).threshold(0.9),
            &mut widget,
        );
        assert!(widget.revealed());
    }

    /// A hidden child paints under a layer at its own opacity, slid down by the
    /// remaining travel; a revealed one paints plainly.
    #[test]
    fn the_reveal_fades_and_slides_its_child_into_place() {
        let hidden = scroll_reveal::<(), _>(child(), 0.0);
        let mut widget = build(&hidden);
        lay_out(&mut widget, BoxConstraints::tight(BOX));

        let (recorder, needs_frame) = painted(&mut widget, BOX, 0, None);
        assert!(!needs_frame, "a settled hidden reveal asks for nothing");
        assert_eq!(recorder.layers, vec![0.0]);
        assert_eq!(
            recorder.transforms[0].as_coeffs()[5],
            DEFAULT_REVEAL_SLIDE,
            "fully hidden sits a whole slide below its place"
        );

        rebuild(&hidden, &scroll_reveal::<(), _>(child(), 0.5), &mut widget);
        let (_, needs_frame) = painted(&mut widget, BOX, 10, None);
        assert!(needs_frame, "a live reveal owes frames");
        let (recorder, _) = painted(&mut widget, BOX, 300, None);
        let alpha = recorder.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "no fade in flight: {alpha}");
        let slide = recorder.transforms[0].as_coeffs()[5];
        assert!(
            slide > 0.0 && slide < DEFAULT_REVEAL_SLIDE,
            "no slide: {slide}"
        );

        // Landed: no layer, no transform, and no further frames.
        let (recorder, needs_frame) = painted(&mut widget, BOX, 1_000, None);
        assert!(!needs_frame);
        assert!(recorder.layers.is_empty() && recorder.transforms.is_empty());
        assert_eq!(widget.shown(), 1.0);
    }

    /// The delay is measured from the first frame that saw the flip, and holds
    /// the child hidden until it elapses.
    #[test]
    fn the_reveal_delay_holds_the_child_back() {
        let hidden = scroll_reveal::<(), _>(child(), 0.0).delay(Duration::from_millis(200));
        let mut widget = build(&hidden);
        lay_out(&mut widget, BoxConstraints::tight(BOX));
        rebuild(
            &hidden,
            &scroll_reveal::<(), _>(child(), 0.5).delay(Duration::from_millis(200)),
            &mut widget,
        );

        // The clock starts at the first frame after the flip, not at the flip.
        let (_, needs_frame) = painted(&mut widget, BOX, 1_000, None);
        assert!(needs_frame, "the delay itself owes frames");
        assert_eq!(widget.shown(), 0.0, "the reveal started during its delay");

        painted(&mut widget, BOX, 1_150, None);
        assert_eq!(widget.shown(), 0.0);

        painted(&mut widget, BOX, 1_200, None);
        painted(&mut widget, BOX, 1_400, None);
        assert!(widget.shown() > 0.0, "the delay never expired");
    }

    /// `reduce_motion` keeps the reveal but drops both the ramp and the slide —
    /// upstream's `hidden = { opacity: 0 }`.
    #[test]
    fn reduce_motion_reveals_without_a_slide() {
        let theme = reduced();
        let hidden = scroll_reveal::<(), _>(child(), 0.0);
        let mut widget = build(&hidden);
        lay_out(&mut widget, BoxConstraints::tight(BOX));
        let (recorder, _) = painted(&mut widget, BOX, 0, Some(&theme));
        assert_eq!(recorder.layers, vec![0.0]);
        assert_eq!(
            recorder.transforms[0].as_coeffs()[5],
            0.0,
            "reduced motion still slid"
        );

        rebuild(&hidden, &scroll_reveal::<(), _>(child(), 0.5), &mut widget);
        let (_, needs_frame) = painted(&mut widget, BOX, 10, Some(&theme));
        assert!(!needs_frame, "reduced motion animated the reveal");
        assert_eq!(widget.shown(), 1.0);
    }

    // ---- ScrollTo ----

    /// Lenis' expo-out curve, anchored at both ends and monotonically rising.
    #[test]
    fn the_scroll_easing_is_lenis_expo_out() {
        assert_eq!(ease_scroll(0.0), 0.0);
        assert_eq!(ease_scroll(1.0), 1.0);
        assert!(ease_scroll(2.0) <= 1.0, "the curve is clamped past its end");
        let mut last = 0.0;
        for step in 1..=100 {
            let t = step as f64 / 100.0;
            let value = ease_scroll(t);
            assert!(value >= last, "not monotonic at {t}");
            assert!((0.0..=1.0).contains(&value));
            last = value;
        }
        // Expo-out front-loads: half-way through the time is most of the way
        // through the travel.
        assert!(
            ease_scroll(0.5) > 0.9,
            "not an expo-out: {}",
            ease_scroll(0.5)
        );
    }

    /// Activation eases the caller's signal from wherever it is to the target,
    /// on upstream's own duration, and lands exactly on it.
    #[test]
    fn activating_eases_the_offset_signal_onto_its_target() {
        let offset = RwSignal::new(900.0);
        let view = scroll_to::<(), _>(child(), 0.0, offset).extra_offset(-40.0);
        let mut widget = build(&view);
        lay_out(&mut widget, BoxConstraints::tight(BOX));
        assert_eq!(widget.target(), -40.0, "the extra offset is added");

        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        let down = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(10.0, 10.0),
            button: PointerButton::Primary,
        });
        let up = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Up,
            position: Point::new(10.0, 10.0),
            button: PointerButton::Primary,
        });
        assert_eq!(widget.event(&mut ctx, &down), EventResult::Handled);
        assert_eq!(widget.event(&mut ctx, &up), EventResult::Handled);
        assert!(widget.is_running());

        painted(&mut widget, BOX, 0, None);
        assert_eq!(
            offset.get_untracked(),
            900.0,
            "the first frame only stamps the clock"
        );
        let (_, needs_frame) = painted(&mut widget, BOX, 300, None);
        assert!(needs_frame, "a live ease owes frames");
        let midway = offset.get_untracked();
        assert!(midway < 900.0 && midway > -40.0, "no ease: {midway}");

        let (_, needs_frame) = painted(&mut widget, BOX, 1_300, None);
        assert!(!needs_frame);
        assert_eq!(offset.get_untracked(), -40.0, "the ease did not land");
        assert!(!widget.is_running());
    }

    /// A release outside the control cancels, and a keyboard activation counts —
    /// the same admission rules every button in this catalog follows.
    #[test]
    fn a_release_outside_cancels_and_a_key_activates() {
        let offset = RwSignal::new(500.0);
        let view = scroll_to::<(), _>(child(), 0.0, offset);
        let mut widget = build(&view);
        lay_out(&mut widget, BoxConstraints::tight(BOX));

        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(10.0, 10.0),
                button: PointerButton::Primary,
            }),
        );
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Up,
                position: Point::new(900.0, 10.0),
                button: PointerButton::Primary,
            }),
        );
        assert!(!widget.is_running(), "a release outside still activated");

        widget.event(
            &mut ctx,
            &InputEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        );
        assert!(widget.is_running(), "Enter did not activate");

        // A disabled control takes nothing at all.
        let disabled = scroll_to::<(), _>(child(), 0.0, offset).disabled(true);
        let mut widget = build(&disabled);
        let result = widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(10.0, 10.0),
                button: PointerButton::Primary,
            }),
        );
        assert_eq!(result, EventResult::Ignored);
    }

    /// Under `reduce_motion` the offset jumps on the first frame rather than
    /// easing — upstream's own "respects reduced motion, jumps instantly".
    #[test]
    fn reduce_motion_jumps_the_offset() {
        let theme = reduced();
        let offset = RwSignal::new(700.0);
        let view = scroll_to::<(), _>(child(), 0.0, offset).duration(Duration::from_millis(600));
        let mut widget = build(&view);
        lay_out(&mut widget, BoxConstraints::tight(BOX));

        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(
            &mut ctx,
            &InputEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        );
        // The event pass carries no theme, so the run is staged either way; the
        // first reduced-motion paint is what lands it, and it asks for nothing
        // further rather than easing over the requested 600ms.
        let (_, needs_frame) = painted(&mut widget, BOX, 0, Some(&theme));
        assert!(!needs_frame, "reduced motion still eased the offset");
        assert_eq!(offset.get_untracked(), 0.0, "reduced motion did not jump");
    }
}
