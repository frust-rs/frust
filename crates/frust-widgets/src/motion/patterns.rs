//! Core transition patterns: the object-safe [`TransitionPattern`] trait plus
//! the four source-verified patterns — [`FadeThrough`], [`SharedAxis`]
//! (X/Y/Scaled), [`FadeScale`], and [`ContainerTransform`] (rect-to-rect
//! morph + fade-through composite) — that
//! [`super::switcher::PatternSwitcher`] stages an outgoing/incoming child pair
//! under. A design system's own patterns live in its own crate
//! (`frust_glyph::motion` is the shipped example) and implement the same
//! public trait from there.
//!
//! # Pure staging math, no widgets
//!
//! A pattern is *pure staging math*: given a `0.0..=1.0` transition progress
//! (`p`), a `reverse` flag, and the container [`Size`], it yields per-child
//! paint parameters ([`PatternLayer`] — `{alpha, dx, dy, scale}`) for the
//! **incoming** and **exiting** children. It never touches a widget, so the
//! staging numbers are table-testable in isolation (mirroring
//! `nav::transition::resolve_layers`'s pure-geometry precedent); the switcher
//! is the sole consumer that applies a resolved [`PatternLayer`] via
//! `push_layer`/`push_transform` and a pod-origin offset.
//!
//! # Progress: raw for position, clamped for opacity
//!
//! `p` is **raw** — a spring [`Timing`](crate::Timing) can overshoot past
//! `1.0`, and that overshoot is applied to *position*/scale so a slide visibly
//! springs past its rest spot. Opacity always uses the `[0, 1]`-clamped value
//! (`push_layer` alpha is never meaningful outside that range) — the same
//! split `nav::transition::resolve_layers` uses.
//!
//! # Source-verified staging numbers
//!
//! Every constant below is either a Material 3 published value or a
//! source-verified Flutter `animations`-package number, cited per constant
//! per `docs/CODE_STANDARDS.md`'s named-constant rule.
//!
//! # Scaffold contract
//!
//! This file owns its own contents only — the [`TransitionPattern`] trait and
//! the four patterns above all live here and never edit `motion/mod.rs`'s
//! module list or re-export block (see that module's docs).
//! The trait's `size` parameter is a deliberate extension point the
//! hero-rect-bearing patterns hang off of, kept in the signature now so a
//! boxed `dyn TransitionPattern` never needs a shape change.

use std::time::Duration;

use frust_core::Curve;
use frust_core::anim::Lerp;
use kurbo::{Point, Rect, Size};

// --- Fade-through staging (source-verified) -------------------

/// Fade-through progress split: the outgoing child finishes its fade-out and
/// the incoming child begins its fade-in + scale-up at this fraction — the
/// verified Flutter `FadeThroughTransition` staging boundary (the first
/// **6/20** of the timeline).
const FADE_THROUGH_SPLIT: f64 = 0.30;

/// Fade-through *outgoing* fade-out easing — Flutter `Cubic(0.4,0,1,1)` applied
/// over `[0, `[`FADE_THROUGH_SPLIT`]`]`, after which the outgoing child holds at
/// `0` opacity.
const FADE_THROUGH_OUT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);

/// Fade-through *incoming* fade-in + scale-up easing — Flutter `Cubic(0,0,0.2,1)`
/// applied over `[`[`FADE_THROUGH_SPLIT`]`, 1]`. Both the
/// opacity `0→1` and the scale [`FADE_THROUGH_SCALE_START`]`→1.0` track this one
/// eased segment.
const FADE_THROUGH_IN_CURVE: Curve = Curve::Cubic(0.0, 0.0, 0.2, 1.0);

/// Fade-through incoming scale start (the incoming child scales 92% → 100% as it
/// fades in) — the verified Flutter `FadeThroughTransition` staging.
const FADE_THROUGH_SCALE_START: f64 = 0.92;

// --- Shared-axis staging (M3 published + Flutter source-verified) ---------------------

/// Shared-axis slide distance, in logical px (dp). Material 3 motion "shared
/// axis" transitions translate by 30dp along the axis. Source: Material Design 3
/// motion guidelines (m3.material.io, "Transitions → Shared axis").
const SHARED_AXIS_SLIDE_DP: f64 = 30.0;

/// Shared-axis "scaled" incoming scale start (92% → 100%). Flutter
/// `SharedAxisTransition(transitionType: scaled)` scales the entering child up
/// from `0.80`; Material 3's own shared-axis-scaled uses a subtler start —
/// `0.80` is the source-verified Flutter value.
const SHARED_AXIS_SCALE_IN_START: f64 = 0.80;

/// Shared-axis "scaled" exiting scale end (100% → 110%): the leaving child
/// scales *up and out* while fading, the source-verified Flutter
/// `SharedAxisTransition` scaled staging.
const SHARED_AXIS_SCALE_OUT_END: f64 = 1.10;

// --- Fade-scale staging (source-verified) --------------------

/// Fade-scale fade interval end: the incoming child's opacity ramps `0→1` over
/// `[0, `[`FADE_SCALE_FADE_END`]`]` (Flutter `FadeScaleTransition`'s
/// `Interval(0, 0.3)`), while its scale runs the full timeline.
const FADE_SCALE_FADE_END: f64 = 0.30;

/// Fade-scale incoming scale start (`0.80 → 1.00` over the full timeline) —
/// Flutter `FadeScaleTransition`.
const FADE_SCALE_SCALE_START: f64 = 0.80;

/// Fade-scale scale easing — Flutter `Easing.legacyDecelerate`
/// (`Cubic(0,0,0.2,1)`).
const FADE_SCALE_SCALE_CURVE: Curve = Curve::Cubic(0.0, 0.0, 0.2, 1.0);

/// Fade-scale default *forward* duration — Flutter `FadeScaleTransition`'s
/// 150ms enter. A source-verified pattern-native constant the
/// switcher's caller can pass to [`.timing(...)`](super::switcher::PatternSwitcherView::timing);
/// the switcher's own default resolves from the theme instead.
pub const FADE_SCALE_FORWARD: Duration = Duration::from_millis(150);

/// Fade-scale default *reverse* duration — Flutter `FadeScaleTransition`'s 75ms
/// exit ("exits always faster than entrances"). See
/// [`FADE_SCALE_FORWARD`].
pub const FADE_SCALE_REVERSE: Duration = Duration::from_millis(75);

// --- The layer + trait ------------------------------------------------------

/// Per-child paint parameters for one frame of a pattern transition: a paint
/// offset (added to the child's pod origin, so paint and hit-testing move
/// together), an opacity (composited via `push_layer`), and a uniform scale
/// (composited via `push_transform` about the child's centre).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatternLayer {
    /// Horizontal paint offset in logical px (added to the child's pod origin).
    pub dx: f64,
    /// Vertical paint offset in logical px (added to the child's pod origin).
    pub dy: f64,
    /// Opacity in `[0, 1]` (composited via `push_layer`).
    pub alpha: f32,
    /// Uniform scale factor about the child's paint centre (composited via
    /// `push_transform`).
    pub scale: f64,
}

impl PatternLayer {
    /// A fully-visible, un-offset, un-scaled layer.
    pub const IDENTITY: PatternLayer = PatternLayer {
        dx: 0.0,
        dy: 0.0,
        alpha: 1.0,
        scale: 1.0,
    };
}

/// Pure staging math for a keyed child switch: maps a `0.0..=1.0` transition
/// progress onto per-child [`PatternLayer`]s for the incoming and exiting
/// children. See the [module docs](self) for the raw-vs-clamped progress
/// contract.
///
/// **Object-safe by contract.** The trait takes only scalar/`Copy` arguments
/// and returns a concrete `(PatternLayer, PatternLayer)`, so any future
/// pattern can hold a `Box<dyn TransitionPattern>` without a signature change —
/// the `size` parameter is the reserved extension point for a hero-rect-bearing
/// container-transform pattern.
pub trait TransitionPattern {
    /// Resolve `(incoming, exiting)` layers at transition progress `p` (raw —
    /// may overshoot for a spring), `reverse` flipping any directional motion,
    /// at container `size`.
    fn resolve(&self, p: f64, reverse: bool, size: Size) -> (PatternLayer, PatternLayer);
}

// A compile-time proof the trait stays object-safe, even though no consumer
// in this crate currently boxes it (`PatternSwitcher` is generic over `P:
// TransitionPattern` instead).
const _: fn() = || {
    let _: Option<&dyn TransitionPattern> = None;
};

/// The eased `(incoming, outgoing)` fade progresses shared by [`FadeThrough`]
/// and [`SharedAxis`]: the outgoing child fades out over `[0, split]` and the
/// incoming child fades in over `[split, 1]`, both from the clamped progress
/// `pc` — Material 3's "fade through" opacity staging.
fn fade_through_progress(pc: f64) -> (f64, f64) {
    let out = FADE_THROUGH_OUT_CURVE
        .interval(0.0, FADE_THROUGH_SPLIT)
        .transform(pc);
    let inc = FADE_THROUGH_IN_CURVE
        .interval(FADE_THROUGH_SPLIT, 1.0)
        .transform(pc);
    (inc, out)
}

// --- FadeThrough ------------------------------------------------------------

/// Material 3 "fade through": the outgoing child fades `1→0`
/// over the first 6/20 of the timeline then holds, while the incoming child
/// holds at [`FADE_THROUGH_SCALE_START`] scale / `0` opacity for that 6/20 then
/// fades in **and** scales up to `1.0` over the remaining 14/20. Non-directional
/// (no slide) — `reverse` has no effect. The idiomatic pattern for a
/// destination change with no spatial relationship (bottom-nav switch, account
/// switch), and the `reduce_motion` collapse target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FadeThrough;

impl TransitionPattern for FadeThrough {
    fn resolve(&self, p: f64, _reverse: bool, _size: Size) -> (PatternLayer, PatternLayer) {
        let pc = p.clamp(0.0, 1.0);
        let (inc, out) = fade_through_progress(pc);
        let incoming = PatternLayer {
            dx: 0.0,
            dy: 0.0,
            alpha: inc as f32,
            scale: FADE_THROUGH_SCALE_START + (1.0 - FADE_THROUGH_SCALE_START) * inc,
        };
        let exiting = PatternLayer {
            dx: 0.0,
            dy: 0.0,
            alpha: (1.0 - out) as f32,
            scale: 1.0,
        };
        (incoming, exiting)
    }
}

// --- SharedAxis -------------------------------------------------------------

/// Which axis (or the scaled variant) a [`SharedAxis`] transition moves along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedAxis {
    /// Horizontal 30dp slide + fade-through opacity (hierarchical forward/back).
    X,
    /// Vertical 30dp slide + fade-through opacity (a step within a flow).
    Y,
    /// A scale (no slide) + fade-through opacity: incoming
    /// [`SHARED_AXIS_SCALE_IN_START`]`→1.0`, exiting `1.0→`[`SHARED_AXIS_SCALE_OUT_END`].
    Scaled,
}

impl TransitionPattern for SharedAxis {
    fn resolve(&self, p: f64, reverse: bool, _size: Size) -> (PatternLayer, PatternLayer) {
        let pc = p.clamp(0.0, 1.0);
        let (inc, out) = fade_through_progress(pc);
        // Direction: forward the incoming child arrives from the positive
        // side; `reverse` mirrors it (a back navigation slides the other way).
        let dir = if reverse { -1.0 } else { 1.0 };
        let slide_in = dir * (1.0 - p) * SHARED_AXIS_SLIDE_DP;
        let slide_out = -dir * p * SHARED_AXIS_SLIDE_DP;

        let inc_alpha = inc as f32;
        let out_alpha = (1.0 - out) as f32;

        match self {
            SharedAxis::X => (
                PatternLayer {
                    dx: slide_in,
                    dy: 0.0,
                    alpha: inc_alpha,
                    scale: 1.0,
                },
                PatternLayer {
                    dx: slide_out,
                    dy: 0.0,
                    alpha: out_alpha,
                    scale: 1.0,
                },
            ),
            SharedAxis::Y => (
                PatternLayer {
                    dx: 0.0,
                    dy: slide_in,
                    alpha: inc_alpha,
                    scale: 1.0,
                },
                PatternLayer {
                    dx: 0.0,
                    dy: slide_out,
                    alpha: out_alpha,
                    scale: 1.0,
                },
            ),
            SharedAxis::Scaled => (
                PatternLayer {
                    dx: 0.0,
                    dy: 0.0,
                    alpha: inc_alpha,
                    scale: SHARED_AXIS_SCALE_IN_START + (1.0 - SHARED_AXIS_SCALE_IN_START) * inc,
                },
                PatternLayer {
                    dx: 0.0,
                    dy: 0.0,
                    alpha: out_alpha,
                    scale: 1.0 + (SHARED_AXIS_SCALE_OUT_END - 1.0) * out,
                },
            ),
        }
    }
}

// --- FadeScale --------------------------------------------------------------

/// Material 3 "fade scale" (Flutter `FadeScaleTransition`): the
/// incoming child fades in over the first 30% of the timeline while scaling
/// [`FADE_SCALE_SCALE_START`]`→1.0` over the full timeline
/// ([`FADE_SCALE_SCALE_CURVE`]); the exiting child **fades only** (no scale).
/// Non-directional — `reverse` has no geometric effect (it selects the faster
/// [`FADE_SCALE_REVERSE`] duration at the switcher's timing layer instead). The
/// idiomatic pattern for a dialog/menu/FAB appearing over stable content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FadeScale;

impl TransitionPattern for FadeScale {
    fn resolve(&self, p: f64, _reverse: bool, _size: Size) -> (PatternLayer, PatternLayer) {
        let pc = p.clamp(0.0, 1.0);
        // Opacity ramps over the first 30% (both children); scale runs the full
        // eased timeline for the incoming child only.
        let fade = Curve::Linear
            .interval(0.0, FADE_SCALE_FADE_END)
            .transform(pc);
        let scale_prog = FADE_SCALE_SCALE_CURVE.transform(pc);
        let incoming = PatternLayer {
            dx: 0.0,
            dy: 0.0,
            alpha: fade as f32,
            scale: FADE_SCALE_SCALE_START + (1.0 - FADE_SCALE_SCALE_START) * scale_prog,
        };
        let exiting = PatternLayer {
            dx: 0.0,
            dy: 0.0,
            alpha: (1.0 - fade) as f32,
            scale: 1.0,
        };
        (incoming, exiting)
    }
}

// --- ContainerTransform (OpenContainer) ----------------------

/// ContainerTransform default duration — the Material 3 container-transform /
/// Flutter `OpenContainer` 300ms morph.
pub const CONTAINER_TRANSFORM_DURATION: Duration = Duration::from_millis(300);

/// The container-transform bounds (rect-morph) easing — Flutter `OpenContainer`
/// tweens its container bounds on `Curves.fastOutSlowIn` (`Cubic(0.4,0,0.2,1)`).
const CONTAINER_TRANSFORM_MORPH_CURVE: Curve = Curve::Cubic(0.4, 0.0, 0.2, 1.0);

/// The plain-`Fade` variant's cross-fade easing — the same decelerate curve the
/// fade-through incoming segment uses (`Cubic(0,0,0.2,1)`), run
/// over the full timeline for both children simultaneously.
const CONTAINER_TRANSFORM_FADE_CURVE: Curve = Curve::Cubic(0.0, 0.0, 0.2, 1.0);

/// How a [`ContainerTransform`] stages opacity while its bounds morph
/// (the fade / fadeThrough variants).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ContainerFade {
    /// Fade-through: the source content fades out over the first
    /// [`FADE_THROUGH_SPLIT`] of the timeline, the incoming content fades in
    /// over the remainder (the M3 default; no simultaneous double-image).
    #[default]
    FadeThrough,
    /// A plain simultaneous cross-fade of both children over the full timeline.
    Fade,
}

/// Material 3 "container transform" (Flutter `OpenContainer`
/// semantics): the incoming content morphs from a captured `source` rect to a
/// `target` rect while opacity fades the source content out and the incoming
/// content in.
///
/// # Pattern-local rect capture (no HeroFrames coupling)
///
/// Unlike `nav::hero`, this pattern carries its own `source`/`target` rects as
/// data (captured from layout by the caller) — it never reaches into the
/// navigator's `HeroFrames` registry, so it composes inside a plain
/// [`super::switcher::PatternSwitcher`] with no core/navigator plumbing. A
/// zero-area `target` is resolved to the full container at `resolve` time (the
/// common "incoming child fills the container" case).
///
/// # v1 morph gap: uniform scale, not a true rect-to-rect affine
///
/// [`PatternLayer`] composites a **uniform** scale about the child's paint
/// centre plus an offset — it cannot express the non-uniform (independent
/// width/height) scale a full rect-to-rect affine needs. So this pattern
/// approximates the morph as a **width-based uniform scale** placed at the
/// interpolated rect's centre: exact when `source`/`target` share an aspect
/// ratio, an approximation otherwise. A true non-uniform morph would need a
/// richer layer type (a `PatternLayer` change rippling into the switcher's
/// `paint_staged_child`), deferred rather than hacked into the v1 contract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContainerTransform {
    /// The source container rect the morph grows from (container-local logical px).
    pub source: Rect,
    /// The target rect the morph settles into; a zero-area rect means "the full
    /// container" (resolved from the `size` passed to [`resolve`](TransitionPattern::resolve)).
    pub target: Rect,
    /// Opacity staging variant.
    pub fade: ContainerFade,
}

impl ContainerTransform {
    /// A fade-through container transform morphing `source → target`.
    pub const fn new(source: Rect, target: Rect) -> Self {
        Self {
            source,
            target,
            fade: ContainerFade::FadeThrough,
        }
    }

    /// A container transform whose incoming child fills the container: `target`
    /// is left zero-area and resolved to the full container size at paint time.
    pub const fn from_source(source: Rect) -> Self {
        Self::new(source, Rect::ZERO)
    }

    /// Select the opacity staging [`ContainerFade`] variant.
    pub const fn with_fade(mut self, fade: ContainerFade) -> Self {
        self.fade = fade;
        self
    }

    /// The interpolated bounds at raw progress `p` (spatial-eased via
    /// [`CONTAINER_TRANSFORM_MORPH_CURVE`]), morphing `source → target` forward
    /// and `target → source` when `reverse`. `target` is used as configured
    /// here — see [`resolve`](TransitionPattern::resolve) for the zero-area
    /// full-container fallback.
    pub fn morph_rect(&self, p: f64, reverse: bool) -> Rect {
        morph_between(self.source, self.target, p, reverse)
    }

    /// `(incoming, exiting)` opacity at clamped progress `pc` for the configured
    /// [`ContainerFade`] variant.
    fn fade_alphas(&self, pc: f64) -> (f64, f64) {
        match self.fade {
            ContainerFade::FadeThrough => {
                let (inc, out) = fade_through_progress(pc);
                (inc, 1.0 - out)
            }
            ContainerFade::Fade => {
                let f = CONTAINER_TRANSFORM_FADE_CURVE.transform(pc);
                (f, 1.0 - f)
            }
        }
    }
}

/// Bounds interpolation shared by [`ContainerTransform::morph_rect`] and its
/// `resolve`: spatial-ease `p`, then lerp `source → target` (or the reverse).
fn morph_between(source: Rect, target: Rect, p: f64, reverse: bool) -> Rect {
    let pe = CONTAINER_TRANSFORM_MORPH_CURVE.transform(p);
    let (from, to) = if reverse {
        (target, source)
    } else {
        (source, target)
    };
    from.lerp(&to, pe)
}

impl TransitionPattern for ContainerTransform {
    fn resolve(&self, p: f64, reverse: bool, size: Size) -> (PatternLayer, PatternLayer) {
        let pc = p.clamp(0.0, 1.0);
        // Zero-area target → the full container (the "incoming fills container"
        // case); otherwise the caller-captured target rect.
        let target = if self.target.width() > 0.0 && self.target.height() > 0.0 {
            self.target
        } else {
            Rect::from_origin_size(Point::ZERO, size)
        };
        let morph = morph_between(self.source, target, p, reverse);

        // Width-based uniform scale about the child centre + a centre offset that
        // places the (container-filling) child within `morph` (see the type's
        // morph-gap note). Guard a degenerate target.
        let scale = if target.width() > 0.0 {
            morph.width() / target.width()
        } else {
            1.0
        };
        let dx = morph.center().x - target.center().x;
        let dy = morph.center().y - target.center().y;

        let (in_a, out_a) = self.fade_alphas(pc);
        let incoming = PatternLayer {
            dx,
            dy,
            alpha: in_a as f32,
            scale,
        };
        let exiting = PatternLayer {
            alpha: out_a as f32,
            ..PatternLayer::IDENTITY
        };
        (incoming, exiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size::new(400.0, 800.0);

    // --- FadeThrough (source-verified staging) ---------------

    #[test]
    fn fade_through_staged_opacity_and_scale_table() {
        // Outgoing fades 1→0 over the first 6/20 (`Cubic(0.4,0,1,1)`) then holds;
        // incoming holds at 0.92 scale / 0 opacity for 6/20 then fades in AND
        // scales to 1.0 over the remaining 14/20 (`Cubic(0,0,0.2,1)`). Split =
        // 6/20 = 0.30. Table at t ∈ {0, 0.3, 0.5, 1.0} for BOTH children.
        let ft = FadeThrough;

        // t = 0: outgoing fully opaque at rest scale; incoming invisible at 0.92.
        let (inc, out) = ft.resolve(0.0, false, SIZE);
        assert_eq!(out.alpha, 1.0);
        assert_eq!(out.scale, 1.0);
        assert_eq!(inc.alpha, 0.0);
        assert!((inc.scale - 0.92).abs() < 1e-9);

        // t = 0.30 (the split): outgoing just finished fading; incoming opens.
        let (inc, out) = ft.resolve(0.30, false, SIZE);
        assert!(out.alpha.abs() < 1e-6, "outgoing gone by the split");
        assert_eq!(inc.alpha, 0.0, "incoming fade-in opens at the split");
        assert!((inc.scale - 0.92).abs() < 1e-9);

        // t = 0.50: outgoing long gone; incoming mid fade-in. Local progress into
        // the [0.30, 1.0] segment is (0.50-0.30)/0.70, eased by `Cubic(0,0,0.2,1)`.
        let (inc, out) = ft.resolve(0.50, false, SIZE);
        assert_eq!(out.alpha, 0.0);
        let eased = Curve::Cubic(0.0, 0.0, 0.2, 1.0).transform((0.50 - 0.30) / 0.70);
        assert!((inc.alpha as f64 - eased).abs() < 1e-6);
        assert!((inc.scale - (0.92 + 0.08 * eased)).abs() < 1e-9);

        // t = 1.0: incoming fully arrived (opaque, unit scale); outgoing gone.
        let (inc, out) = ft.resolve(1.0, false, SIZE);
        assert_eq!(inc.alpha, 1.0);
        assert!((inc.scale - 1.0).abs() < 1e-9);
        assert_eq!(out.alpha, 0.0);
        assert_eq!(out.scale, 1.0);
    }

    #[test]
    fn fade_through_is_non_directional_reverse_equals_forward() {
        for t in [0.0, 0.3, 0.5, 1.0] {
            assert_eq!(
                FadeThrough.resolve(t, false, SIZE),
                FadeThrough.resolve(t, true, SIZE),
                "fade-through has no direction: reverse must match forward at t={t}"
            );
        }
    }

    // --- SharedAxis (X/Y/Scaled) --------------------------------------------

    #[test]
    fn shared_axis_x_slides_and_crossfades_forward_and_reverse() {
        let sx = SharedAxis::X;
        // Forward start: incoming offset by the full slide, invisible; exiting at
        // rest, fully opaque.
        let (inc, out) = sx.resolve(0.0, false, SIZE);
        assert_eq!(inc.dx, SHARED_AXIS_SLIDE_DP);
        assert_eq!(inc.alpha, 0.0);
        assert_eq!(out.dx, 0.0);
        assert_eq!(out.alpha, 1.0);
        // Forward end: incoming at rest & opaque; exiting slid out & transparent.
        let (inc, out) = sx.resolve(1.0, false, SIZE);
        assert_eq!(inc.dx, 0.0);
        assert_eq!(inc.alpha, 1.0);
        assert_eq!(out.dx, -SHARED_AXIS_SLIDE_DP);
        assert_eq!(out.alpha, 0.0);
        // Reverse mirrors the horizontal direction.
        let (inc, _out) = sx.resolve(0.0, true, SIZE);
        assert_eq!(inc.dx, -SHARED_AXIS_SLIDE_DP);
        // Fade split at t=0.30: exiting gone, incoming still invisible.
        let (inc, out) = sx.resolve(0.30, false, SIZE);
        assert!(out.alpha.abs() < 1e-6);
        assert_eq!(inc.alpha, 0.0);
        // Opacity uses the same eased fade-through progress as FadeThrough.
        let (fi, _fo) = SharedAxis::X.resolve(0.5, false, SIZE);
        let (fti, _fto) = FadeThrough.resolve(0.5, false, SIZE);
        assert!((fi.alpha - fti.alpha).abs() < 1e-9);
    }

    #[test]
    fn shared_axis_y_slides_vertically_only() {
        let (inc, out) = SharedAxis::Y.resolve(0.0, false, SIZE);
        assert_eq!(inc.dx, 0.0);
        assert_eq!(inc.dy, SHARED_AXIS_SLIDE_DP);
        assert_eq!(out.dy, 0.0);
        let (inc, out) = SharedAxis::Y.resolve(1.0, false, SIZE);
        assert_eq!(inc.dy, 0.0);
        assert_eq!(out.dy, -SHARED_AXIS_SLIDE_DP);
    }

    #[test]
    fn shared_axis_scaled_scales_without_sliding() {
        let s = SharedAxis::Scaled;
        // t=0: incoming at 0.80 scale/invisible; exiting at 1.0/opaque.
        let (inc, out) = s.resolve(0.0, false, SIZE);
        assert_eq!(inc.dx, 0.0);
        assert_eq!(inc.dy, 0.0);
        assert!((inc.scale - SHARED_AXIS_SCALE_IN_START).abs() < 1e-9);
        assert_eq!(inc.alpha, 0.0);
        assert_eq!(out.scale, 1.0);
        assert_eq!(out.alpha, 1.0);
        // t=1: incoming rests at 1.0; exiting scaled up to 1.10, transparent.
        let (inc, out) = s.resolve(1.0, false, SIZE);
        assert!((inc.scale - 1.0).abs() < 1e-9);
        assert_eq!(inc.alpha, 1.0);
        assert!((out.scale - SHARED_AXIS_SCALE_OUT_END).abs() < 1e-9);
        assert_eq!(out.alpha, 0.0);
    }

    // --- FadeScale (source-verified staging) -----------------

    #[test]
    fn fade_scale_incoming_fades_early_scales_full_exiting_fades_only() {
        let fs = FadeScale;
        // t=0: incoming invisible at 0.80 scale; exiting opaque, unscaled.
        let (inc, out) = fs.resolve(0.0, false, SIZE);
        assert_eq!(inc.alpha, 0.0);
        assert!((inc.scale - FADE_SCALE_SCALE_START).abs() < 1e-9);
        assert_eq!(out.alpha, 1.0);
        assert_eq!(out.scale, 1.0);

        // t=0.30: fade interval closes — incoming opaque, exiting gone; scale
        // still mid-flight (< 1.0) on the incoming child.
        let (inc, out) = fs.resolve(0.30, false, SIZE);
        assert!((inc.alpha - 1.0).abs() < 1e-6, "fade completes by 0.30");
        assert!(out.alpha.abs() < 1e-6);
        let scale_at_30 = FADE_SCALE_SCALE_START
            + (1.0 - FADE_SCALE_SCALE_START) * FADE_SCALE_SCALE_CURVE.transform(0.30);
        assert!((inc.scale - scale_at_30).abs() < 1e-9);
        assert!(inc.scale < 1.0, "scale is still animating at t=0.30");

        // t=0.50: still opaque, scale continuing.
        let (inc, _out) = fs.resolve(0.50, false, SIZE);
        assert!((inc.alpha - 1.0).abs() < 1e-6);
        let scale_at_50 = FADE_SCALE_SCALE_START
            + (1.0 - FADE_SCALE_SCALE_START) * FADE_SCALE_SCALE_CURVE.transform(0.50);
        assert!((inc.scale - scale_at_50).abs() < 1e-9);

        // t=1.0: incoming fully settled; exiting fully faded, never scaled.
        let (inc, out) = fs.resolve(1.0, false, SIZE);
        assert!((inc.alpha - 1.0).abs() < 1e-6);
        assert!((inc.scale - 1.0).abs() < 1e-9);
        assert_eq!(out.alpha, 0.0);
        assert_eq!(
            out.scale, 1.0,
            "the exiting child never scales (fades only)"
        );
    }

    #[test]
    fn fade_scale_reverse_is_geometrically_identical() {
        // Reverse selects the faster duration at the switcher's timing layer, not
        // a different geometry — the staging math is identical.
        for t in [0.0, 0.3, 0.5, 1.0] {
            assert_eq!(
                FadeScale.resolve(t, false, SIZE),
                FadeScale.resolve(t, true, SIZE),
                "fade-scale geometry is direction-independent at t={t}"
            );
        }
    }

    #[test]
    fn forward_default_durations_encode_exits_faster_than_entrances() {
        assert_eq!(FADE_SCALE_FORWARD, Duration::from_millis(150));
        assert_eq!(FADE_SCALE_REVERSE, Duration::from_millis(75));
        assert!(
            FADE_SCALE_REVERSE < FADE_SCALE_FORWARD,
            "exits always faster than entrances"
        );
    }

    // --- ContainerTransform (OpenContainer) ------------------

    const SRC: Rect = Rect::new(100.0, 200.0, 200.0, 400.0); // 100x200 @ (100,200)

    #[test]
    fn container_transform_rect_interpolation_at_fixed_t() {
        // Morph a small source rect into the full container; the interpolated
        // bounds ease `source → target` on the fastOutSlowIn curve.
        let ct = ContainerTransform::new(SRC, Rect::from_origin_size(Point::ZERO, SIZE));
        let target = Rect::from_origin_size(Point::ZERO, SIZE);

        // t=0: exactly the source rect.
        let r0 = ct.morph_rect(0.0, false);
        assert!((r0.x0 - SRC.x0).abs() < 1e-9 && (r0.y0 - SRC.y0).abs() < 1e-9);
        assert!((r0.width() - SRC.width()).abs() < 1e-9);

        // t=1: exactly the target (full container).
        let r1 = ct.morph_rect(1.0, false);
        assert!((r1.width() - SIZE.width).abs() < 1e-9);
        assert!((r1.height() - SIZE.height).abs() < 1e-9);

        // t=0.5: the eased lerp of source→target (fastOutSlowIn at 0.5).
        let pe = CONTAINER_TRANSFORM_MORPH_CURVE.transform(0.5);
        let r5 = ct.morph_rect(0.5, false);
        let expect = SRC.lerp(&target, pe);
        assert!((r5.x0 - expect.x0).abs() < 1e-9);
        assert!((r5.width() - expect.width()).abs() < 1e-9);

        // reverse morphs target → source.
        let rr = ct.morph_rect(0.0, true);
        assert!(
            (rr.width() - SIZE.width).abs() < 1e-9,
            "reverse starts at target"
        );
    }

    #[test]
    fn container_transform_zero_area_target_resolves_to_full_container() {
        // `from_source` leaves the target zero-area; resolve fills the container.
        let ct = ContainerTransform::from_source(SRC);
        // t=1: incoming settled at unit scale / no offset (fills the container).
        let (inc, _out) = ct.resolve(1.0, false, SIZE);
        assert!((inc.scale - 1.0).abs() < 1e-9);
        assert!(inc.dx.abs() < 1e-9 && inc.dy.abs() < 1e-9);
        // t=0: incoming shrunk to the source width fraction, centred on source.
        let (inc0, _out0) = ct.resolve(0.0, false, SIZE);
        assert!((inc0.scale - SRC.width() / SIZE.width).abs() < 1e-9);
        let full = Rect::from_origin_size(Point::ZERO, SIZE);
        assert!((inc0.dx - (SRC.center().x - full.center().x)).abs() < 1e-9);
        assert!((inc0.dy - (SRC.center().y - full.center().y)).abs() < 1e-9);
    }

    #[test]
    fn container_transform_fade_through_opacity_staging() {
        let ct = ContainerTransform::from_source(SRC); // FadeThrough default
        // t=0: source fully opaque, incoming invisible.
        let (inc, out) = ct.resolve(0.0, false, SIZE);
        assert_eq!(out.alpha, 1.0);
        assert_eq!(inc.alpha, 0.0);
        // At the split: source gone, incoming still opening.
        let (inc, out) = ct.resolve(FADE_THROUGH_SPLIT, false, SIZE);
        assert!(out.alpha.abs() < 1e-6);
        assert_eq!(inc.alpha, 0.0);
        // t=1: incoming opaque, source gone.
        let (inc, out) = ct.resolve(1.0, false, SIZE);
        assert!((inc.alpha - 1.0).abs() < 1e-6);
        assert_eq!(out.alpha, 0.0);
    }

    #[test]
    fn container_transform_plain_fade_variant_cross_fades_simultaneously() {
        let ct = ContainerTransform::from_source(SRC).with_fade(ContainerFade::Fade);
        // Both children cross-fade together: alphas sum to ~1 across the timeline.
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (inc, out) = ct.resolve(t, false, SIZE);
            assert!(
                (inc.alpha + out.alpha - 1.0).abs() < 1e-6,
                "plain fade cross-fades (alphas sum to 1) at t={t}"
            );
        }
    }

    #[test]
    fn transition_patterns_reduce_motion_collapse_target_removes_motion() {
        // The switcher collapses ANY pattern to a fast FadeThrough
        // crossfade under reduce_motion. Prove the collapse *removes spatial
        // motion* — FadeThrough (the collapse target) is non-directional (no
        // slide) where a translating pattern moves, at a shared mid-progress.
        // (FadeThrough keeps its own subtle scale-up; the removed motion is the
        // directional slide/morph offset.) `SharedAxis` and
        // `ContainerTransform` are the two in-crate translating patterns; an
        // out-of-crate pattern (e.g. `frust_glyph::motion::GlyphSlide`)
        // collapses identically, since the switcher substitutes the pattern
        // wholesale rather than asking it to reduce itself.
        let (ft_in, _) = FadeThrough.resolve(0.5, false, SIZE);
        assert_eq!(ft_in.dx, 0.0);
        assert_eq!(ft_in.dy, 0.0);

        let (axis_in, _) = SharedAxis::X.resolve(0.5, false, SIZE);
        assert!(
            axis_in.dx.abs() > 0.0,
            "SharedAxis offsets where the collapse does not"
        );

        let (ct_in, _) = ContainerTransform::from_source(SRC).resolve(0.5, false, SIZE);
        assert!(
            (ct_in.scale - 1.0).abs() > 1e-6 || ct_in.dx.abs() > 0.0 || ct_in.dy.abs() > 0.0,
            "ContainerTransform morphs where the collapse does not"
        );
    }
}
