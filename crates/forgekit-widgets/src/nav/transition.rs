//! Page-transition machinery for the [`navigator`](super::navigator) (Phase 6b,
//! task 03): the transition vocabulary ([`PageTransition`] presets + [`Timing`]
//! modes), the per-transition progress [`driver`](TransitionDriver) the navigator
//! advances during paint, and the pure geometry ([`resolve_layers`]) that maps a
//! progress value onto per-page paint offsets + opacities.
//!
//! # Split of concerns
//!
//! This module is the **reusable, page-agnostic** half of the transition system:
//! it knows nothing about the retained page stack. The navigator
//! ([`super::navigator`]) owns the [`ActiveTransition`](super::navigator) that
//! pairs a [`TransitionDriver`] with the concrete incoming/outgoing page pods and
//! drives it from `PaintCtx::frame_time` during paint — see that module for the
//! ownership/lifecycle contract (create per push/pop, dispose on settle, Flutter
//! parity).
//!
//! # Positioning rule (paint offset = pod origin)
//!
//! A slide animates a page's **pod origin** (the navigator writes
//! [`ChildPod::set_origin`](forgekit_core::ChildPod::set_origin) each paint), so
//! paint and hit-testing move together — the same precedent `ScrollWidget` uses.
//! Opacity is a paint-only effect (`PaintScene::push_layer(alpha)`); it may
//! diverge from hit-testing, which is irrelevant because the navigator blocks all
//! input to pages while a transition runs (see [`super::navigator`]).
//!
//! # Overshoot: spatial vs effects
//!
//! A [`Timing::Spring`] mode drives the progress with an
//! [`AnimationController::fling`](forgekit_core::AnimationController::fling): an
//! under-damped **spatial** preset (M3's `damping_ratio: 0.9`) genuinely
//! overshoots past `1.0` before settling, and that overshoot is applied to
//! *position* offsets (raw [`value`](TransitionDriver::value)) so the page visibly
//! springs past its resting spot. **Opacity** uses the clamped value so alpha
//! never exceeds `[0, 1]` — a critically-damped effects preset never overshoots
//! anyway, but clamping is the belt-and-braces guarantee (see
//! `forgekit-core`'s `anim` overshoot contract).

use std::time::Duration;

use forgekit_core::{AnimationController, Curve, FrameTime, Spring, SpringDesc};
use forgekit_theme::MotionSpring;
use kurbo::Size;

// --- Named preset constants (see per-constant source comments) --------------

/// M3 shared-axis-X slide distance, in logical px (dp). Confirmed spec value:
/// Material 3 motion "shared axis" transitions translate by 30dp along the axis.
/// Source: Material Design 3 motion guidelines (m3.material.io, "Transitions →
/// Shared axis").
const M3_SHARED_AXIS_SLIDE_DP: f64 = 30.0;

/// Progress split between the outgoing fade-out and the incoming fade-in for
/// M3 shared-axis and fade-through transitions: the outgoing page fades out over
/// `[0, THRESHOLD]` and the incoming page fades in over `[THRESHOLD, 1]`.
///
/// M3's "fade through" is defined by *progress fractions* (a fade-out then a
/// fade-in with a brief gap), not fixed millisecond offsets — see the phase-6b
/// refuted-claims ledger #3. ~0.35 is the split the reference implementations
/// use.
const M3_FADE_SPLIT: f64 = 0.35;

/// M3 fade-through incoming scale start (the incoming page scales 92% → 100% as
/// it fades in). Source: Material Design 3 "fade through" spec.
///
/// **Documented, not applied**: a scale is a scene-transform effect, and the
/// current `PaintScene` seam (task 6a) exposes only `push_layer`(alpha)/`push_clip`
/// — no per-widget transform. Applying it would require extending the layer-2
/// `PaintScene` trait, which is outside this task's scope; the fade-through
/// transition therefore ships as a pure cross-fade for now, with the scale
/// recorded here for when the transform seam lands.
#[allow(dead_code)]
const M3_FADE_THROUGH_SCALE_START: f64 = 0.92;

/// iOS push parallax fraction: the outgoing (below) page slides out by one third
/// of the incoming page's travel while the incoming page slides fully across.
///
/// **Community-approximate** (see phase-6b refuted-claims ledger #2): UIKit's
/// `UINavigationController` push does not publish an exact parallax ratio; 1/3 is
/// the value the community-reverse-engineered reimplementations converge on.
const IOS_PARALLAX_FRACTION: f64 = 1.0 / 3.0;

/// Maximum dim applied to the outgoing (below) page during an iOS push — its
/// opacity drops to `1 - IOS_DIM_MAX` at full cover.
///
/// **Customary, not stock** (ledger #2): a dim scrim under the incoming page is a
/// common embellishment, not a documented UIKit constant. Kept small and
/// approximate.
const IOS_DIM_MAX: f32 = 0.08;

/// iOS push default duration. **Community-approximate** (~0.35s ease-in-out);
/// UIKit's exact interactive-transition timing is private (ledger #2).
const IOS_DEFAULT_DURATION: Duration = Duration::from_millis(350);

/// The default duration for a duration-mode M3 transition (300ms). Source:
/// Material Design 3 motion durations ("long2" ≈ the 300ms shared-axis default).
const M3_DEFAULT_DURATION: Duration = Duration::from_millis(300);

/// The spring used to *settle* a transition that was driven manually (task 05's
/// edge-swipe) but configured in a duration [`Timing`] mode — a duration has no
/// spring to fling with, so a release needs a fallback. M3's default **spatial**
/// preset (`damping_ratio: 0.9`, `stiffness: 700`, mass 1). Source:
/// material-components-android motion tokens (see `forgekit-theme`'s `motion`).
const DEFAULT_SETTLE_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 700.0,
    damping_ratio: 0.9,
};

// --- Public vocabulary ------------------------------------------------------

/// The visual shape of a page transition. The navigator maps the active
/// transition's progress onto per-page geometry through [`resolve_layers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageTransition {
    /// Instant switch — no animation (the task-02 behavior). The default.
    #[default]
    None,
    /// Material 3 shared-axis-X: a 30dp slide paired with a threshold cross-fade.
    M3SharedAxisX,
    /// Material 3 fade-through: outgoing fade-out then incoming fade-in (a scale
    /// component is spec'd but not yet applied — see [`M3_FADE_THROUGH_SCALE_START`]).
    M3FadeThrough,
    /// iOS-style push/pop: incoming slides full-width from the edge; outgoing
    /// parallaxes by [`IOS_PARALLAX_FRACTION`] with an optional dim.
    IosPush,
    /// Bottom-sheet-style slide-up: the entering page translates in from the
    /// bottom (full height → 0); pop reverses (slides back down and out). The
    /// page **below** never moves (a modal sheet floats over a static page,
    /// unlike [`PageTransition::IosPush`]'s parallaxing below-page). Opacity is
    /// always `1.0` for both layers — a sheet's scrim is a **page-owned** paint
    /// concern (the hosting widget paints/fades its own scrim, e.g. reading the
    /// transition progress itself or a fixed alpha), not a [`Layer`]-level
    /// effect this preset drives; see [`resolve_layers`]'s `SlideUp` arm.
    SlideUp,
}

/// How a transition's `0.0..=1.0` progress is driven.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timing {
    /// Duration + easing curve (bounded, no overshoot).
    Duration(Duration, Curve),
    /// A physics spring ([`MotionSpring`], from the theme's motion scheme). A
    /// spatial preset overshoots position; an effects preset does not.
    Spring(MotionSpring),
}

impl Default for Timing {
    fn default() -> Self {
        Timing::Duration(M3_DEFAULT_DURATION, Curve::EaseInOut)
    }
}

/// A transition selection: which [`PageTransition`] shape, driven by which
/// [`Timing`]. Attached per-push/replace (or defaulted at the navigator level);
/// a pop reverses the popped page's stored spec.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionSpec {
    /// The visual preset.
    pub preset: PageTransition,
    /// How its progress is driven.
    pub timing: Timing,
}

impl Default for TransitionSpec {
    fn default() -> Self {
        Self::NONE
    }
}

impl TransitionSpec {
    /// An instant (non-animated) switch — the navigator's default.
    pub const NONE: Self = TransitionSpec {
        preset: PageTransition::None,
        timing: Timing::Duration(M3_DEFAULT_DURATION, Curve::EaseInOut),
    };

    /// A transition with an explicit preset and timing.
    pub const fn new(preset: PageTransition, timing: Timing) -> Self {
        TransitionSpec { preset, timing }
    }

    /// A duration-driven transition with the preset's natural default duration
    /// and an ease-in-out curve.
    pub fn duration(preset: PageTransition) -> Self {
        let d = match preset {
            PageTransition::IosPush => IOS_DEFAULT_DURATION,
            // SlideUp is a Material-family surface (a bottom sheet), not an iOS
            // one, so it follows the same M3 "long2" 300ms default the other
            // two Material presets use here — a duration-mode default, not a
            // theme spring, purely to keep this table uniform; an app wanting a
            // bouncier sheet can still opt into `TransitionSpec::spring` with
            // any `MotionSpring` preset (e.g. the theme's `default_spatial`),
            // same as the other presets.
            _ => M3_DEFAULT_DURATION,
        };
        TransitionSpec {
            preset,
            timing: Timing::Duration(d, Curve::EaseInOut),
        }
    }

    /// A spring-driven transition using a theme [`MotionSpring`] preset (use a
    /// *spatial* preset for a visible overshoot, an *effects* preset for none).
    pub fn spring(preset: PageTransition, spring: MotionSpring) -> Self {
        TransitionSpec {
            preset,
            timing: Timing::Spring(spring),
        }
    }

    /// Whether this spec animates at all (`false` for [`PageTransition::None`]).
    pub fn is_animated(&self) -> bool {
        self.preset != PageTransition::None
    }
}

// --- Progress driver --------------------------------------------------------

/// The result of advancing a [`TransitionDriver`] one frame.
#[derive(Clone, Copy, Debug)]
pub struct Advance {
    /// The progress value this frame (may exceed `[0, 1]` mid-overshoot for a
    /// spatial spring).
    pub value: f64,
    /// Whether the driver is still moving (the caller should request another
    /// frame).
    pub animating: bool,
    /// Whether the driver has reached its resting target this frame (the
    /// navigator finalizes the transition on the next rebuild).
    pub done: bool,
}

/// Drives a transition's `0.0..=1.0` progress. The programmatic push/pop path
/// uses [`Auto`](Self::Auto) (an [`AnimationController`] advanced during paint);
/// the [`Held`](Self::Held)/[`Settle`](Self::Settle) variants are the seam task
/// 05's edge-swipe gesture drives (`set_progress`/`settle` on the navigator).
#[derive(Clone, Copy, Debug)]
pub enum TransitionDriver {
    /// Programmatic drive: an [`AnimationController`] (duration or spring fling)
    /// advanced from the frame clock.
    Auto(AnimationController),
    /// Externally pinned progress (task 05 drag-in-progress): paint reads `value`
    /// verbatim and never advances; the transition stays alive (paused).
    Held { value: f64 },
    /// A released spring settle (task 05 fling): an analytic [`Spring`] released
    /// from the held value toward `target`, advanced by frame-time differencing.
    Settle {
        spring: Spring,
        target: f64,
        elapsed: f64,
        last: Option<FrameTime>,
    },
}

impl TransitionDriver {
    /// The current progress value (raw — may overshoot for a spatial spring).
    pub fn value(&self) -> f64 {
        match self {
            TransitionDriver::Auto(c) => c.value(),
            TransitionDriver::Held { value } => *value,
            TransitionDriver::Settle {
                spring,
                target,
                elapsed,
                ..
            } => target + spring.position(*elapsed),
        }
    }

    /// Advance to frame time `now`, returning this frame's [`Advance`].
    pub fn advance(&mut self, now: FrameTime) -> Advance {
        match self {
            TransitionDriver::Auto(c) => {
                let animating = c.advance(now);
                Advance {
                    value: c.value(),
                    animating,
                    done: !animating,
                }
            }
            TransitionDriver::Held { value } => Advance {
                value: *value,
                animating: false,
                done: false,
            },
            TransitionDriver::Settle {
                spring,
                target,
                elapsed,
                last,
            } => {
                let dt = match *last {
                    Some(prev) => now.saturating_sub(prev).as_secs_f64(),
                    None => 0.0,
                };
                *last = Some(now);
                *elapsed += dt.max(0.0);
                let e = *elapsed;
                if spring.is_at_rest(e, 1e-3) {
                    Advance {
                        value: *target,
                        animating: false,
                        done: true,
                    }
                } else {
                    Advance {
                        value: *target + spring.position(e),
                        animating: true,
                        done: false,
                    }
                }
            }
        }
    }
}

/// Build a fresh [`TransitionDriver`] (already running `0 → 1`) plus the spring
/// to use for a later manual settle, from a [`Timing`].
pub fn make_driver(timing: Timing) -> (TransitionDriver, SpringDesc) {
    match timing {
        Timing::Duration(d, curve) => {
            let mut c = AnimationController::new(d).with_curve(curve);
            c.forward();
            (TransitionDriver::Auto(c), DEFAULT_SETTLE_SPRING)
        }
        Timing::Spring(spring) => {
            let desc: SpringDesc = spring.into();
            // Duration is irrelevant for a fling; the controller starts at 0 and
            // flings toward 1 with zero release velocity (a spatial preset still
            // overshoots — that is the point of a bouncy transition).
            let mut c = AnimationController::new(M3_DEFAULT_DURATION);
            c.fling(0.0, desc);
            (TransitionDriver::Auto(c), desc)
        }
    }
}

/// Build a [`TransitionDriver::Settle`] that springs from `from` toward `target`
/// with initial `velocity` — the navigator's `settle(velocity)` seam (task 05).
pub fn settle_driver(
    spring: SpringDesc,
    from: f64,
    velocity: f64,
    target: f64,
) -> TransitionDriver {
    TransitionDriver::Settle {
        spring: Spring::new(spring, from - target, velocity),
        target,
        elapsed: 0.0,
        last: None,
    }
}

// --- Geometry ---------------------------------------------------------------

/// Per-page paint parameters for one frame of a transition: a paint offset
/// (applied to the page's pod origin) and an opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    /// Horizontal paint offset in logical px (added to the page's pod origin).
    pub dx: f64,
    /// Vertical paint offset in logical px (added to the page's pod origin).
    /// Every preset before [`PageTransition::SlideUp`] is horizontal-only and
    /// leaves this at `0.0`.
    pub dy: f64,
    /// Opacity in `[0, 1]` (composited via `PaintScene::push_layer`).
    pub alpha: f32,
}

impl Layer {
    /// A fully-visible, un-offset layer.
    pub const IDENTITY: Layer = Layer {
        dx: 0.0,
        dy: 0.0,
        alpha: 1.0,
    };
}

/// Resolve the (entering, leaving) [`Layer`]s for a transition preset at progress
/// `value`.
///
/// - `entering` is the page that becomes top after the op (a push's new page, or
///   a pop's revealed page); `leaving` is the page losing the top.
/// - `value` is raw (used for **position**, so a spatial spring's overshoot shows
///   in the slide); opacity uses the `[0, 1]`-clamped value.
/// - `is_pop` reverses the horizontal direction (a pop slides the opposite way).
pub fn resolve_layers(
    preset: PageTransition,
    value: f64,
    is_pop: bool,
    size: Size,
) -> (Layer, Layer) {
    let p = value; // raw (overshoot allowed) — used for position
    let pc = value.clamp(0.0, 1.0); // clamped — used for opacity
    let w = size.width;

    match preset {
        PageTransition::None => (Layer::IDENTITY, Layer::IDENTITY),

        PageTransition::M3SharedAxisX => {
            let slide = M3_SHARED_AXIS_SLIDE_DP;
            // Push: entering enters from +30dp; pop: from -30dp (mirror).
            let dir = if is_pop { -1.0 } else { 1.0 };
            let entering = Layer {
                dx: dir * (1.0 - p) * slide,
                dy: 0.0,
                alpha: ramp(pc, M3_FADE_SPLIT, 1.0),
            };
            let leaving = Layer {
                dx: -dir * p * slide,
                dy: 0.0,
                alpha: 1.0 - ramp(pc, 0.0, M3_FADE_SPLIT),
            };
            (entering, leaving)
        }

        PageTransition::M3FadeThrough => {
            // Pure cross-fade (scale spec'd but not applied — see the constant).
            let entering = Layer {
                dx: 0.0,
                dy: 0.0,
                alpha: ramp(pc, M3_FADE_SPLIT, 1.0),
            };
            let leaving = Layer {
                dx: 0.0,
                dy: 0.0,
                alpha: 1.0 - ramp(pc, 0.0, M3_FADE_SPLIT),
            };
            (entering, leaving)
        }

        PageTransition::IosPush => {
            if is_pop {
                // Revealed page slides back from -parallax to 0; popped page
                // slides fully off to the right.
                let entering = Layer {
                    dx: -(1.0 - p) * w * IOS_PARALLAX_FRACTION,
                    dy: 0.0,
                    alpha: 1.0,
                };
                let leaving = Layer {
                    dx: p * w,
                    dy: 0.0,
                    alpha: 1.0,
                };
                (entering, leaving)
            } else {
                // Incoming slides full-width from the right; below page
                // parallaxes left and dims.
                let entering = Layer {
                    dx: (1.0 - p) * w,
                    dy: 0.0,
                    alpha: 1.0,
                };
                let leaving = Layer {
                    dx: -p * w * IOS_PARALLAX_FRACTION,
                    dy: 0.0,
                    alpha: 1.0 - pc as f32 * IOS_DIM_MAX,
                };
                (entering, leaving)
            }
        }

        PageTransition::SlideUp => {
            let h = size.height;
            if is_pop {
                // The revealed page below never moved while covered (see the
                // enum docs) — it stays at rest, full opacity, the whole time.
                // The popped sheet (leaving) slides from rest back down and out.
                let entering = Layer {
                    dx: 0.0,
                    dy: 0.0,
                    alpha: 1.0,
                };
                let leaving = Layer {
                    dx: 0.0,
                    dy: p * h,
                    alpha: 1.0,
                };
                (entering, leaving)
            } else {
                // The entering sheet slides up from the bottom (full height
                // offset) to rest; the page below stays static and fully
                // opaque throughout (no parallax/dim, unlike `IosPush`).
                let entering = Layer {
                    dx: 0.0,
                    dy: (1.0 - p) * h,
                    alpha: 1.0,
                };
                let leaving = Layer {
                    dx: 0.0,
                    dy: 0.0,
                    alpha: 1.0,
                };
                (entering, leaving)
            }
        }
    }
}

/// A linear ramp: `0` at `start`, `1` at `end`, clamped outside. Used for the
/// threshold cross-fades. `start == end` degenerates to a step at `start`.
fn ramp(t: f64, start: f64, end: f64) -> f32 {
    if end <= start {
        return if t >= start { 1.0 } else { 0.0 };
    }
    (((t - start) / (end - start)).clamp(0.0, 1.0)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    const SIZE: Size = Size::new(400.0, 800.0);

    #[test]
    fn ramp_is_clamped_linear() {
        assert_eq!(ramp(0.0, 0.35, 1.0), 0.0);
        assert_eq!(ramp(0.35, 0.35, 1.0), 0.0);
        assert_eq!(ramp(1.0, 0.35, 1.0), 1.0);
        assert!((ramp(0.675, 0.35, 1.0) - 0.5).abs() < 1e-6);
        // Degenerate window → step.
        assert_eq!(ramp(0.1, 0.5, 0.5), 0.0);
        assert_eq!(ramp(0.9, 0.5, 0.5), 1.0);
    }

    #[test]
    fn shared_axis_slides_and_crossfades_forward() {
        // At the start, entering is offset by the full slide and invisible;
        // leaving is at rest and fully opaque.
        let (enter, leave) = resolve_layers(PageTransition::M3SharedAxisX, 0.0, false, SIZE);
        assert_eq!(enter.dx, M3_SHARED_AXIS_SLIDE_DP);
        assert_eq!(enter.alpha, 0.0);
        assert_eq!(leave.dx, 0.0);
        assert_eq!(leave.alpha, 1.0);

        // At the end, entering rests at 0 and is fully opaque; leaving is fully
        // slid out and transparent.
        let (enter, leave) = resolve_layers(PageTransition::M3SharedAxisX, 1.0, false, SIZE);
        assert_eq!(enter.dx, 0.0);
        assert_eq!(enter.alpha, 1.0);
        assert_eq!(leave.dx, -M3_SHARED_AXIS_SLIDE_DP);
        assert_eq!(leave.alpha, 0.0);
    }

    #[test]
    fn shared_axis_pop_mirrors_direction() {
        let (enter, _leave) = resolve_layers(PageTransition::M3SharedAxisX, 0.0, true, SIZE);
        // Pop: entering comes from the *left* (negative offset).
        assert_eq!(enter.dx, -M3_SHARED_AXIS_SLIDE_DP);
    }

    #[test]
    fn ios_push_incoming_full_width_and_outgoing_parallax() {
        let (enter, leave) = resolve_layers(PageTransition::IosPush, 0.0, false, SIZE);
        // Incoming starts one full width to the right; below page at rest.
        assert_eq!(enter.dx, SIZE.width);
        assert_eq!(leave.dx, 0.0);

        let (enter, leave) = resolve_layers(PageTransition::IosPush, 1.0, false, SIZE);
        assert_eq!(enter.dx, 0.0);
        // Below page parallaxes by 1/3 width and is dimmed.
        assert!((leave.dx + SIZE.width * IOS_PARALLAX_FRACTION).abs() < 1e-9);
        assert!(leave.alpha < 1.0);
    }

    #[test]
    fn fade_through_is_pure_crossfade() {
        let (enter, leave) = resolve_layers(PageTransition::M3FadeThrough, 0.5, false, SIZE);
        assert_eq!(enter.dx, 0.0);
        assert_eq!(leave.dx, 0.0);
    }

    #[test]
    fn slide_up_enters_from_bottom_and_settles() {
        // At the start, the entering sheet sits a full height below rest, fully
        // visible (opacity is a page-owned scrim concern, not this preset's).
        let (enter, leave) = resolve_layers(PageTransition::SlideUp, 0.0, false, SIZE);
        assert_eq!(enter.dx, 0.0);
        assert_eq!(enter.dy, SIZE.height);
        assert_eq!(enter.alpha, 1.0);
        // The page below never moves or fades.
        assert_eq!(leave.dx, 0.0);
        assert_eq!(leave.dy, 0.0);
        assert_eq!(leave.alpha, 1.0);

        // At the end, the sheet rests at dy = 0; the below page is unchanged.
        let (enter, leave) = resolve_layers(PageTransition::SlideUp, 1.0, false, SIZE);
        assert_eq!(enter.dy, 0.0);
        assert_eq!(enter.alpha, 1.0);
        assert_eq!(leave.dy, 0.0);
        assert_eq!(leave.alpha, 1.0);
    }

    #[test]
    fn slide_up_pop_reverses_and_never_moves_below_page() {
        // Pop: the sheet (leaving) slides back down; the revealed page
        // (entering) stays static at rest throughout.
        let (enter, leave) = resolve_layers(PageTransition::SlideUp, 0.0, true, SIZE);
        assert_eq!(enter.dy, 0.0);
        assert_eq!(leave.dy, 0.0);

        let (enter, leave) = resolve_layers(PageTransition::SlideUp, 1.0, true, SIZE);
        assert_eq!(enter.dy, 0.0, "revealed page never moves");
        assert_eq!(
            leave.dy, SIZE.height,
            "the sheet slides fully off the bottom"
        );
        assert_eq!(enter.alpha, 1.0);
        assert_eq!(leave.alpha, 1.0, "SlideUp never fades a page's own layer");
    }

    #[test]
    fn duration_driver_runs_zero_to_one_and_settles() {
        let (mut driver, _) =
            make_driver(Timing::Duration(Duration::from_millis(100), Curve::Linear));
        // Seed the clock (zero delta).
        let a = driver.advance(ft_secs(0.0));
        assert!(a.animating && !a.done);
        assert!((a.value - 0.0).abs() < 1e-9);
        // Halfway.
        let a = driver.advance(ft_secs(0.05));
        assert!((a.value - 0.5).abs() < 1e-6);
        // Past the end: settles at 1.0, reports done.
        let a = driver.advance(ft_secs(0.2));
        assert!(!a.animating && a.done);
        assert_eq!(a.value, 1.0);
    }

    #[test]
    fn spring_spatial_driver_overshoots_past_one() {
        // M3 default spatial: damping 0.9 — overshoots.
        let spring = MotionSpring {
            damping_ratio: 0.9,
            stiffness: 700.0,
        };
        let (mut driver, _) = make_driver(Timing::Spring(spring));
        let mut max = f64::MIN;
        let mut t = 0.0;
        for _ in 0..100_000 {
            let a = driver.advance(ft_secs(t));
            max = max.max(a.value);
            if a.done {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(
            max > 1.0 + 1e-3,
            "spatial spring should overshoot past 1.0, got {max}"
        );
    }

    #[test]
    fn spring_effects_driver_never_overshoots() {
        // M3 default effects: damping 1.0 — critically damped, released from
        // rest, so it approaches 1.0 monotonically.
        let spring = MotionSpring {
            damping_ratio: 1.0,
            stiffness: 1600.0,
        };
        let (mut driver, _) = make_driver(Timing::Spring(spring));
        let mut t = 0.0;
        for _ in 0..100_000 {
            let a = driver.advance(ft_secs(t));
            assert!(
                a.value <= 1.0 + 1e-9,
                "effects spring overshot: {}",
                a.value
            );
            if a.done {
                break;
            }
            t += 1.0 / 120.0;
        }
    }

    #[test]
    fn held_driver_pauses_without_finishing() {
        let mut driver = TransitionDriver::Held { value: 0.4 };
        let a = driver.advance(ft_secs(1.0));
        assert_eq!(a.value, 0.4);
        assert!(!a.animating);
        assert!(!a.done, "a held driver never reports done (drag paused)");
    }

    #[test]
    fn settle_driver_springs_to_target() {
        let mut driver = settle_driver(DEFAULT_SETTLE_SPRING, 0.6, 0.0, 1.0);
        let mut t = 0.0;
        let mut done = false;
        for _ in 0..100_000 {
            let a = driver.advance(ft_secs(t));
            if a.done {
                done = true;
                assert_eq!(a.value, 1.0);
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(done, "settle driver failed to reach its target");
    }
}
