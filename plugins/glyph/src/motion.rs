//! The two Glyph motion patterns: [`GlyphSlide`] (directional 16dp slide +
//! fade, a `TransitionPattern` a `PatternSwitcher` stages a child pair under)
//! and [`GlyphStagger`] (per-item log-reveal, a plain per-item helper).
//!
//! # Pure staging math, no widgets
//!
//! A pattern is *pure staging math*: given a `0.0..=1.0` transition progress
//! (`p`), a `reverse` flag, and the container `Size`, it yields per-child paint
//! parameters (`frust::motion::patterns::PatternLayer` — `{alpha, dx, dy,
//! scale}`) for the **incoming** and **exiting** children. It never touches a
//! widget, so the staging numbers are table-testable in isolation; the
//! switcher is the sole consumer that applies a resolved layer.
//!
//! # Progress: raw for position, clamped for opacity
//!
//! `p` is **raw** — a spring `Timing` can overshoot past `1.0`, and that
//! overshoot is applied to *position*/scale so a slide visibly springs past its
//! rest spot. Opacity always uses the `[0, 1]`-clamped value (`push_layer`
//! alpha is never meaningful outside that range).
//!
//! # Source-verified staging numbers
//!
//! Every constant below is a source-verified Glyph value, cited per constant
//! per `docs/CODE_STANDARDS.md`'s named-constant rule.

use std::time::Duration;

use frust::Curve;
use frust::authoring::Size;
use frust::motion::patterns::{PatternLayer, TransitionPattern};

// --- GlyphSlide -----------------------------------------

/// GlyphSlide travel distance, 16 logical px (dp). The Glyph directional
/// enter/exit slide distance.
const GLYPH_SLIDE_DISTANCE_DP: f64 = 16.0;

/// GlyphSlide enter duration — a 340ms spatial slide-in + fade. A
/// pattern-native constant the switcher's caller can pass to that switcher's
/// `.timing(...)`; the theme's Glyph `MotionScheme` resolves the effective
/// timing otherwise.
pub const GLYPH_SLIDE_ENTER: Duration = Duration::from_millis(340);

/// GlyphSlide exit duration — a 150ms exit-curve slide-out;
/// exits run faster than entrances. See [`GLYPH_SLIDE_ENTER`].
pub const GLYPH_SLIDE_EXIT: Duration = Duration::from_millis(150);

/// GlyphSlide enter spatial easing — the Glyph `spatial` curve, matching
/// `MotionScheme::m3_expressive`'s `easing.spatial` (`Cubic(0.05,0.7,0.1,1)`);
/// the unthemed fallback for the slide-in motion.
const GLYPH_SLIDE_SPATIAL: Curve = Curve::Cubic(0.05, 0.7, 0.1, 1.0);

/// GlyphSlide enter fade easing — the Glyph `effects` curve
/// (`Cubic(0.2,0,0,1)`; opacity, never overshoots).
const GLYPH_SLIDE_EFFECTS: Curve = Curve::Cubic(0.2, 0.0, 0.0, 1.0);

/// GlyphSlide exit easing — the Glyph `exit` curve (`Cubic(0.3,0,1,1)`;
/// accelerates leaving content out), driving both the slide-out offset and the
/// exit fade.
const GLYPH_SLIDE_EXIT_CURVE: Curve = Curve::Cubic(0.3, 0.0, 1.0, 1.0);

/// The leading fraction of the shared transition timeline the (faster) exit
/// slide-out completes within — 150ms exit over the 340ms enter timeline — so
/// the outgoing child clears before the incoming child settles. Mirrors the
/// fade-through split idiom the core patterns use.
const GLYPH_SLIDE_EXIT_FRACTION: f64 = 150.0 / 340.0;

/// Which way a [`GlyphSlide`] entering child travels (the incoming child moves
/// *toward* this edge; `reverse` mirrors it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideDirection {
    /// Travels upward: the incoming child enters from below and rises into place.
    Up,
    /// Travels downward: the incoming child enters from above and drops into place.
    Down,
    /// Travels left: the incoming child enters from the right.
    Left,
    /// Travels right: the incoming child enters from the left.
    Right,
}

impl SlideDirection {
    /// Unit travel vector `(x, y)` in logical-px space (y-down).
    fn unit(self) -> (f64, f64) {
        match self {
            SlideDirection::Up => (0.0, -1.0),
            SlideDirection::Down => (0.0, 1.0),
            SlideDirection::Left => (-1.0, 0.0),
            SlideDirection::Right => (1.0, 0.0),
        }
    }
}

/// Glyph's directional slide: the incoming child slides in
/// [`GLYPH_SLIDE_DISTANCE_DP`] along [`SlideDirection`] over the enter timeline
/// while fading in; the outgoing child slides out the same way over the faster
/// [`GLYPH_SLIDE_EXIT_FRACTION`] of the timeline while fading out. `reverse`
/// mirrors the travel direction (a "back" switch).
///
/// # reduce_motion
///
/// Like every `TransitionPattern`, the `reduce_motion` collapse is applied by
/// the `PatternSwitcher` staging it, which substitutes a fast linear
/// fade-through crossfade (no slide) for the whole pattern when the theme's
/// `MotionScheme::reduce_motion` is set — so a slide never plays under reduced
/// motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphSlide {
    /// The direction the incoming child travels.
    pub direction: SlideDirection,
}

impl GlyphSlide {
    /// A slide in the given direction.
    pub const fn new(direction: SlideDirection) -> Self {
        Self { direction }
    }
}

impl TransitionPattern for GlyphSlide {
    fn resolve(&self, p: f64, reverse: bool, _size: Size) -> (PatternLayer, PatternLayer) {
        let pc = p.clamp(0.0, 1.0);
        let (mut ux, mut uy) = self.direction.unit();
        if reverse {
            ux = -ux;
            uy = -uy;
        }

        // Incoming: enters over the full timeline — starts offset OPPOSITE the
        // travel direction by the full distance and eases to rest, fading in.
        let enter = GLYPH_SLIDE_SPATIAL.transform(pc);
        let in_off = (1.0 - enter) * GLYPH_SLIDE_DISTANCE_DP;
        let incoming = PatternLayer {
            dx: -ux * in_off,
            dy: -uy * in_off,
            alpha: GLYPH_SLIDE_EFFECTS.transform(pc) as f32,
            scale: 1.0,
        };

        // Exiting: a faster slide-out compressed into the leading fraction of the
        // timeline — continues in the travel direction, fading on the exit curve.
        let exit_local = (pc / GLYPH_SLIDE_EXIT_FRACTION).min(1.0);
        let exit = GLYPH_SLIDE_EXIT_CURVE.transform(exit_local);
        let out_off = exit * GLYPH_SLIDE_DISTANCE_DP;
        let exiting = PatternLayer {
            dx: ux * out_off,
            dy: uy * out_off,
            alpha: (1.0 - exit) as f32,
            scale: 1.0,
        };

        (incoming, exiting)
    }
}

// --- GlyphStagger ------------------------------------------------------

/// GlyphStagger per-item reveal duration — a 150ms `effects`-curve fade-in
/// (the reference build's rendered value).
const GLYPH_STAGGER_ITEM_MS: f64 = 150.0;

/// GlyphStagger inter-item delay — 90ms. **Prose-vs-rendered discrepancy**:
/// Glyph's spec *prose* cites a different figure, but 90ms is the value
/// the reference build actually *renders*; the rendered value wins here.
const GLYPH_STAGGER_DELAY_MS: f64 = 90.0;

/// GlyphStagger per-item easing — the Glyph `effects` curve (`Cubic(0.2,0,0,1)`;
/// an opacity reveal, never overshoots).
const GLYPH_STAGGER_CURVE: Curve = Curve::Cubic(0.2, 0.0, 0.0, 1.0);

/// Glyph's staggered log-reveal: each item fades in over
/// [`GLYPH_STAGGER_ITEM_MS`], successive items offset by
/// [`GLYPH_STAGGER_DELAY_MS`], all driven off one shared `0.0..=1.0` controller
/// value — no per-item controller.
///
/// Unlike a `TransitionPattern`, a stagger reveals an N-item *list*, so it
/// isn't expressible as the trait's `(incoming, exiting)` pair. It is instead a
/// plain per-item helper: a list-entrance container asks for each item's
/// [`item_layer`](Self::item_layer), and unrelated widgets (a
/// [`TermBlock`](crate::TermBlock), a boot sequence) drive the same
/// [`item_progress`](Self::item_progress) directly.
///
/// The three timing fields carry `frust-core`'s staggered-animation idiom
/// ("item `i` starts at `i * per_item_delay` and runs for `item_duration`")
/// **inline** rather than through a `StaggerSpec` value: that core type is not
/// reachable through the `frust` facade, and this plugin depends on the facade
/// alone. `per_item_delay`/`item_duration` share whatever time unit the caller
/// picks, as long as it matches [`total_duration`](Self::total_duration)'s
/// consumer (e.g. an `AnimationController`'s `Duration`).
///
/// # reduce_motion
///
/// Pass `reduce_motion = true` to collapse the stagger: every item reveals
/// together on the shared progress (no per-item delay) — a single fast fade
/// instead of a cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphStagger {
    /// Time between the start of consecutive items' sub-animations.
    pub per_item_delay: f64,
    /// Duration of each item's own sub-animation.
    pub item_duration: f64,
    /// Easing curve applied within each item's local `[0, 1]` sub-progress.
    pub item_curve: Curve,
}

impl GlyphStagger {
    /// The Glyph-baseline stagger: 90ms per-item delay, 150ms `effects`-curve
    /// per-item reveal.
    pub const fn glyph() -> Self {
        Self {
            per_item_delay: GLYPH_STAGGER_DELAY_MS,
            item_duration: GLYPH_STAGGER_ITEM_MS,
            item_curve: GLYPH_STAGGER_CURVE,
        }
    }

    /// The full timeline length (in the spec's ms unit) for `n` items — size an
    /// `AnimationController`'s forward duration off this so one value drives
    /// every item. The last item starts at `(n - 1) * per_item_delay` and runs
    /// `item_duration`. `n == 0` has no timeline: `0.0`.
    pub fn total_duration(&self, n: usize) -> f64 {
        if n == 0 {
            return 0.0;
        }
        (n as f64 - 1.0) * self.per_item_delay + self.item_duration
    }

    /// Item `i`'s eased reveal progress in `[0, 1]` at the shared controller
    /// `overall` (clamped to `[0, 1]`, linear across
    /// [`total_duration`](Self::total_duration)`(n)`), for a list of `n` items.
    ///
    /// Before item `i`'s local window starts this is `0.0`; after it ends,
    /// `1.0`; in between it re-evaluates `item_curve` at the item's local
    /// `[0, 1]` sub-progress. A zero-length `item_duration` degenerates to an
    /// instantaneous step at the item's start time. `n == 0` (no timeline) and
    /// `i >= n` both have no defined window, so this returns `1.0` (nothing
    /// left to reveal). `reduce_motion` collapses the cascade so every item
    /// tracks `overall` directly (no per-item delay).
    pub fn item_progress(&self, overall: f64, i: usize, n: usize, reduce_motion: bool) -> f64 {
        if reduce_motion {
            return overall.clamp(0.0, 1.0);
        }
        let total = self.total_duration(n);
        if total <= 0.0 || i >= n {
            return 1.0;
        }
        let overall = overall.clamp(0.0, 1.0);
        let elapsed = overall * total;
        let item_start = i as f64 * self.per_item_delay;
        let item_end = item_start + self.item_duration;
        if self.item_duration <= 0.0 {
            return if elapsed < item_start { 0.0 } else { 1.0 };
        }
        if elapsed <= item_start {
            0.0
        } else if elapsed >= item_end {
            1.0
        } else {
            let local = (elapsed - item_start) / self.item_duration;
            self.item_curve.transform(local)
        }
    }

    /// Item `i`'s reveal `PatternLayer` — an opacity-only fade-in (a log/boot
    /// line appearing), with no spatial offset. See
    /// [`item_progress`](Self::item_progress) for the `reduce_motion` collapse.
    pub fn item_layer(
        &self,
        overall: f64,
        i: usize,
        n: usize,
        reduce_motion: bool,
    ) -> PatternLayer {
        PatternLayer {
            alpha: self.item_progress(overall, i, n, reduce_motion) as f32,
            ..PatternLayer::IDENTITY
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size::new(400.0, 800.0);

    // --- GlyphSlide ------------------------------------

    #[test]
    fn glyph_slide_up_offsets_and_alphas_forward_and_reverse() {
        let gs = GlyphSlide::new(SlideDirection::Up);

        // Forward t=0: incoming enters from below (dy = +16), invisible; exiting
        // at rest, opaque.
        let (inc, out) = gs.resolve(0.0, false, SIZE);
        assert!(
            (inc.dy - GLYPH_SLIDE_DISTANCE_DP).abs() < 1e-9,
            "enters from below"
        );
        assert_eq!(inc.dx, 0.0);
        assert_eq!(inc.alpha, 0.0);
        assert_eq!(out.dy, 0.0);
        assert_eq!(out.alpha, 1.0);

        // Forward t=1: incoming settled (dy=0, opaque); exiting slid out the top
        // (dy = -16) and faded.
        let (inc, out) = gs.resolve(1.0, false, SIZE);
        assert!(inc.dy.abs() < 1e-9);
        assert!((inc.alpha - 1.0).abs() < 1e-6);
        assert!(
            (out.dy + GLYPH_SLIDE_DISTANCE_DP).abs() < 1e-9,
            "exits to the top"
        );
        assert_eq!(out.alpha, 0.0);

        // Forward t=0.5: incoming partway (offset shrinking, alpha rising); exit
        // already complete (150/340 fraction ≈ 0.44 < 0.5), fully gone.
        let (inc, out) = gs.resolve(0.5, false, SIZE);
        let enter = GLYPH_SLIDE_SPATIAL.transform(0.5);
        assert!((inc.dy - (1.0 - enter) * GLYPH_SLIDE_DISTANCE_DP).abs() < 1e-9);
        assert!(inc.alpha > 0.0 && inc.alpha < 1.0);
        assert!(out.alpha.abs() < 1e-6, "faster exit completes before t=0.5");
        assert!((out.dy + GLYPH_SLIDE_DISTANCE_DP).abs() < 1e-9);

        // Reverse mirrors the vertical direction (enters from above instead).
        let (inc_r, _out_r) = gs.resolve(0.0, true, SIZE);
        assert!(
            (inc_r.dy + GLYPH_SLIDE_DISTANCE_DP).abs() < 1e-9,
            "reverse enters from above"
        );
    }

    #[test]
    fn glyph_slide_horizontal_moves_on_x_only() {
        let (inc, out) = GlyphSlide::new(SlideDirection::Left).resolve(0.0, false, SIZE);
        assert_eq!(inc.dy, 0.0);
        assert_eq!(out.dy, 0.0);
        // Left travel: incoming enters from the right (dx = +16).
        assert!((inc.dx - GLYPH_SLIDE_DISTANCE_DP).abs() < 1e-9);
    }

    #[test]
    fn glyph_slide_exit_is_faster_than_enter() {
        assert_eq!(GLYPH_SLIDE_ENTER, Duration::from_millis(340));
        assert_eq!(GLYPH_SLIDE_EXIT, Duration::from_millis(150));
        assert!(GLYPH_SLIDE_EXIT < GLYPH_SLIDE_ENTER);
        assert!((GLYPH_SLIDE_EXIT_FRACTION - 150.0 / 340.0).abs() < 1e-12);
    }

    // --- GlyphStagger -----------------

    #[test]
    fn glyph_stagger_item_progress_matches_the_stagger_math_for_five_items() {
        let gs = GlyphStagger::glyph();
        let n = 5;
        // Independent re-derivation of the staggered-reveal idiom: item `i`
        // opens at `i * delay` and runs `item_duration`, eased by the curve.
        let total = (n as f64 - 1.0) * GLYPH_STAGGER_DELAY_MS + GLYPH_STAGGER_ITEM_MS;
        for &overall in &[0.0, 0.2, 0.5, 0.8, 1.0] {
            for i in 0..n {
                let elapsed = overall * total;
                let start = i as f64 * GLYPH_STAGGER_DELAY_MS;
                let expected = if elapsed <= start {
                    0.0
                } else if elapsed >= start + GLYPH_STAGGER_ITEM_MS {
                    1.0
                } else {
                    GLYPH_STAGGER_CURVE.transform((elapsed - start) / GLYPH_STAGGER_ITEM_MS)
                };
                assert!(
                    (gs.item_progress(overall, i, n, false) - expected).abs() < 1e-12,
                    "GlyphStagger reveal math at overall={overall}, i={i}"
                );
            }
        }

        // Hand-computed anchor: 5 items, delay=90, dur=150 → total = 4*90+150 =
        // 510ms. Item 2 opens at 180ms, closes at 330ms. At overall=0.5 →
        // elapsed=255ms → local=(255-180)/150=0.5, eased by the effects curve.
        assert!((gs.total_duration(5) - 510.0).abs() < 1e-9);
        let eased = GLYPH_STAGGER_CURVE.transform(0.5);
        assert!((gs.item_progress(0.5, 2, 5, false) - eased).abs() < 1e-9);
        // Item 0 has fully revealed by then; the last item hasn't opened.
        assert_eq!(gs.item_progress(0.5, 0, 5, false), 1.0);
        assert_eq!(gs.item_progress(0.5, 4, 5, false), 0.0);
    }

    #[test]
    fn glyph_stagger_reduce_motion_collapses_to_simultaneous_fade() {
        let gs = GlyphStagger::glyph();
        // Every item tracks `overall` directly — no per-item delay.
        for i in 0..5 {
            assert!((gs.item_progress(0.5, i, 5, true) - 0.5).abs() < 1e-12);
        }
        // The collapse is observable: the last item is 0.0 staggered but 0.5
        // collapsed at overall=0.5.
        assert_eq!(gs.item_progress(0.5, 4, 5, false), 0.0);
        assert!((gs.item_progress(0.5, 4, 5, true) - 0.5).abs() < 1e-12);
        // item_layer carries the collapsed alpha, identity elsewhere.
        let layer = gs.item_layer(0.5, 4, 5, true);
        assert!((layer.alpha - 0.5).abs() < 1e-6);
        assert_eq!(layer.dx, 0.0);
        assert_eq!(layer.scale, 1.0);
    }

    #[test]
    fn glyph_stagger_degenerate_counts_have_nothing_left_to_reveal() {
        let gs = GlyphStagger::glyph();
        assert_eq!(gs.total_duration(0), 0.0);
        assert_eq!(gs.item_progress(0.5, 0, 0, false), 1.0);
        assert_eq!(gs.item_progress(0.5, 3, 3, false), 1.0);
    }
}
