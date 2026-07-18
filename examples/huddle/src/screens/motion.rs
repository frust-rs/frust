//! Showcase · Motion screen — REAL (wave-2 task 04, ported from
//! `examples/gallery`).
//!
//! A duration+[`Curve`] tween and the M3 Expressive `default-spatial`
//! (bounce) vs `default-effects` (no bounce) spring presets side by side —
//! `examples/gallery`'s motion page (spec §18), **entirely** driven by
//! [`AnimationController`] (`advance(ctx.frame_time())` + `request_frame`,
//! per `docs/CODE_STANDARDS.md`'s Theming & Animation Conventions). Unlike
//! the gallery, the spring demos also go through
//! [`AnimationController::fling`] rather than a raw self-driving [`Spring`]
//! — this retires the `docs/CODE_STANDARDS.md`-documented raw-`Spring`
//! exception for good (task 10 updates that doc).
//!
//! # Local retained state
//!
//! The trailing "Replay" button needs somewhere to keep a restart counter
//! across rebuilds. Rather than reaching into [`crate::ShellState`] (out of
//! this task's file scope), [`MotionScreenComponent`] hosts the page as a
//! tiny local [`Component`] (`State = u64`, the CODE_STANDARDS-documented
//! "local state lives in the retained `Component` element" pattern) —
//! `crate::routes`'s `/` route already establishes this same
//! `any(component(..))` shape for `TeamScreen`.
//!
//! # Why this file depends on `forgekit-core`/`kurbo`/`peniko` directly
//!
//! No facade widget paints a paint-driven "puck sliding along a track", so
//! [`MotionBoxView`] below is the same small `View`/`Widget` pair
//! `examples/gallery` built directly against `forgekit-core` (spec §6) — see
//! `examples/gallery/src/lib.rs`'s module docs for the precedent, and this
//! crate's `screens::theme` module for the sibling escape-hatch widget.

use std::time::Duration;

use forgekit::{
    AnimationController, AnyView, Button, ColorScheme, Column, Component, Curve, MotionSpring,
    SizedBox, SpringDesc, Theme, any, component, scroll_view, text, use_context,
};
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Point, Size};
use peniko::Color;

use crate::ShellState;

// --- MotionBoxView/Widget: a puck sliding along a track, frame-clock-driven ---

/// Which motion primitive drives a [`MotionBoxView`]'s puck: a duration +
/// [`Curve`] tween, or a physics [`SpringDesc`] fling — both run entirely
/// through [`AnimationController`], never a raw [`forgekit::Spring`].
#[derive(Clone, Copy)]
enum MotionKind {
    Curve(Curve, Duration),
    Spring(SpringDesc),
}

/// Initial fling velocity (value-units/second) for the spring demos — picked
/// so an under-damped preset (M3's `*-spatial` springs) visibly overshoots
/// before settling, and a critically-damped one (`*-effects`) visibly does
/// not.
const FLING_VELOCITY: f64 = 3.0;

/// A placeholder duration the fling variant's [`AnimationController`] is
/// constructed with — irrelevant to a spring fling (only duration+`Curve`
/// motion reads it), kept tiny so it reads as inert at a glance.
const FLING_PLACEHOLDER_DURATION: Duration = Duration::from_millis(1);

/// An animated "puck sliding along a track", driven by the shell frame clock
/// (spec §8) during its own paint — the gallery's motion-page primitive,
/// entirely re-based on [`AnimationController`] (both the tween and the
/// spring fling). See the module docs for why this is a hand-rolled
/// `View`/`Widget` pair.
struct MotionBoxView {
    kind: MotionKind,
    trigger: u64,
    track: Size,
    puck: f64,
    track_color: Color,
    puck_color: Color,
}

fn motion_box(
    kind: MotionKind,
    trigger: u64,
    track: Size,
    puck: f64,
    track_color: Color,
    puck_color: Color,
) -> MotionBoxView {
    MotionBoxView {
        kind,
        trigger,
        track,
        puck,
        track_color,
        puck_color,
    }
}

struct MotionBoxWidget {
    kind: MotionKind,
    trigger: u64,
    track: Size,
    puck: f64,
    track_color: Color,
    puck_color: Color,
    controller: AnimationController,
}

impl MotionBoxWidget {
    /// (Re)start the motion from the controller's rest position, seeding a
    /// fresh [`AnimationController`] drive so a "Replay" tap always shows
    /// the full motion again.
    fn restart(&mut self) {
        self.controller = match self.kind {
            MotionKind::Curve(curve, duration) => {
                let mut controller = AnimationController::new(duration).with_curve(curve);
                controller.forward();
                controller
            }
            MotionKind::Spring(spring) => {
                let mut controller = AnimationController::new(FLING_PLACEHOLDER_DURATION);
                // Settles toward 1.0 (positive velocity); an under-damped
                // `spring` genuinely overshoots past it before settling —
                // `value()` (not `value_clamped()`) is read in `paint` below
                // so that overshoot stays visible, exactly like the gallery
                // demo this replaces.
                controller.fling(FLING_VELOCITY, spring);
                controller
            }
        };
    }
}

impl<State: 'static> View<State> for MotionBoxView {
    type Element = MotionBoxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MotionBoxWidget {
        let mut widget = MotionBoxWidget {
            kind: self.kind,
            trigger: self.trigger,
            track: self.track,
            puck: self.puck,
            track_color: self.track_color,
            puck_color: self.puck_color,
            // Placeholder, immediately replaced by `restart()` below.
            controller: AnimationController::new(FLING_PLACEHOLDER_DURATION),
        };
        widget.restart();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MotionBoxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.track = self.track;
        element.puck = self.puck;
        element.track_color = self.track_color;
        element.puck_color = self.puck_color;
        if self.trigger != prev.trigger {
            element.kind = self.kind;
            element.trigger = self.trigger;
            element.restart();
        }
        ChangeFlags::PAINT
    }
}

impl Widget for MotionBoxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.track)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let animating = self.controller.advance(ctx.frame_time());
        let origin = ctx.origin();
        let size = ctx.size();
        scene.fill_rounded_rect(origin, size, size.height / 2.0, self.track_color);

        let value = self.controller.value();
        let travel = (size.width - self.puck).max(0.0);
        let x = (origin.x + value * travel).clamp(
            origin.x - self.puck * 0.2,
            origin.x + size.width - self.puck * 0.8,
        );
        scene.fill_rounded_rect(
            Point::new(x, origin.y),
            Size::new(self.puck, size.height),
            size.height / 2.0,
            self.puck_color,
        );

        if animating {
            ctx.request_frame();
        }
    }
}

// --- The motion exhibit ---

/// `forgekit-theme::MotionSpring` (mass implicitly `1.0`, see its module
/// docs) into `forgekit-core::anim`'s `SpringDesc`, the generic physics type
/// [`AnimationController::fling`] consumes.
fn spring_desc(spring: MotionSpring) -> SpringDesc {
    SpringDesc {
        mass: 1.0,
        stiffness: spring.stiffness,
        damping_ratio: spring.damping_ratio,
    }
}

/// The motion page: a duration+[`Curve::Emphasized`] tween, and the M3
/// Expressive `default-spatial` (bounce) vs `default-effects` (no bounce)
/// spring presets side by side — both restart on the trailing "Replay"
/// button, which bumps `trigger`.
fn motion_page(theme: &Theme, scheme: ColorScheme, trigger: u64) -> Vec<AnyView<u64>> {
    let track = Size::new(320.0, 48.0);
    let mut v: Vec<AnyView<u64>> = Vec::new();

    v.push(any(text("Motion").size(28.0).color(scheme.on_surface)));
    v.push(any(text(
        "Duration+curve tweens and M3 spring presets, all driven by AnimationController.",
    )
    .size(14.0)
    .color(scheme.on_surface)));
    v.push(any(SizedBox(None, Some(12.0))));

    v.push(any(text("Duration + curve (Emphasized)")
        .size(18.0)
        .color(scheme.on_surface)));
    v.push(any(motion_box(
        MotionKind::Curve(Curve::Emphasized, Duration::from_millis(900)),
        trigger,
        track,
        48.0,
        scheme.surface_container_highest,
        scheme.primary,
    )));

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(text(
        "Spring: default-spatial (bounce) vs default-effects (no bounce)",
    )
    .size(18.0)
    .color(scheme.on_surface)));
    v.push(any(motion_box(
        MotionKind::Spring(spring_desc(theme.motion.default_spatial)),
        trigger,
        track,
        48.0,
        scheme.surface_container_highest,
        scheme.primary,
    )));
    v.push(any(SizedBox(None, Some(8.0))));
    v.push(any(motion_box(
        MotionKind::Spring(spring_desc(theme.motion.default_effects)),
        trigger,
        track,
        48.0,
        scheme.surface_container_highest,
        scheme.tertiary,
    )));

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(Button("Replay", |s: &mut u64| {
        *s = s.wrapping_add(1);
    })));

    v
}

/// The motion screen's tiny local [`Component`] (see the module docs): its
/// whole `State` is the "Replay" trigger counter [`MotionBoxWidget::restart`]
/// watches for.
struct MotionScreenComponent;

impl Component for MotionScreenComponent {
    type State = u64;

    fn init(&self) -> u64 {
        0
    }

    fn build(&self, state: &mut u64) -> AnyView<u64> {
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let scheme = *theme.scheme();
        any(scroll_view(Column(motion_page(&theme, scheme, *state))))
    }
}

/// The motion exhibit (see the module docs).
pub fn motion_screen() -> AnyView<ShellState> {
    any(component(MotionScreenComponent))
}
