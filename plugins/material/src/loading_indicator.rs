//! The Material 3 Expressive loading indicator: a continuously-morphing,
//! continuously-rotating sequence of seven Material shapes, driven by the
//! crate's [`Morph`] feature-point engine.
//!
//! # Attribution
//!
//! Ported from `paadevelopments/material_3_expressive` v1.0.8 (MIT, © 2026
//! Paa Developments), `lib/components/loading_indicator/` — whose
//! `components/m3e_expressive_loading_indicator.dart` is itself, per that
//! file's own header, vendored **verbatim** from the `loading_indicator_m3e`
//! package, a Dart port of AndroidX Compose Material3's
//! `LoadingIndicator.kt`
//! (`androidx/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/LoadingIndicator.kt`,
//! © 2024 The Android Open Source Project, Apache License 2.0). The shape
//! sequence, timing constants, spring, and rotation choreography below are
//! transcribed from that Apache-2.0-licensed logic, retargeted onto this
//! crate's own `shapes::Morph`/`RoundedPolygon` engine rather than Flutter's.
//! The container variant and its theme defaults
//! (`lib/components/loading_indicator/{m3e_loading_indicator,styles/m3e_loading_indicator_theme}.dart`)
//! are the MIT-licensed `material_3_expressive` wrapper around it. See
//! `plugins/material/NOTICE`'s Apache License, Version 2.0 — The Android
//! Open Source Project section (this module activates its
//! previously-`planned` loading-indicator entry).
//!
//! # Shape sequence
//!
//! [`SHAPE_CYCLE`] is upstream's `_defaultPolygons`
//! (`m3e_expressive_loading_indicator.dart:57-65`): `softBurst`,
//! `cookie9Sided`, `pentagon`, `pill`, `sunny`, `cookie4Sided`, `oval`, each
//! resolved through this crate's 35-shape [`ShapeKind`] catalog. [`morph_sequence`]
//! builds the circular run of [`Morph`]s between consecutive shapes
//! (`_createMorphSequence(polygons, circularSequence: true)`,
//! `m3e_expressive_loading_indicator.dart:195-211`) once and shares it across
//! every indicator instance — building a `Morph` runs the expensive
//! corner-measure/match pipeline (see [`Morph`]'s own cost note).
//!
//! # Choreography
//!
//! Two independent clocks drive the paint, matching
//! `m3e_expressive_loading_indicator.dart:96-166`'s `build`:
//!
//! - **Per-cycle morph.** Every [`PER_SHAPE_PERIOD`] (650ms — upstream's
//!   `_morphIntervalMs`), the indicator advances to the next morph in
//!   [`morph_sequence`] and restarts a spring-driven progress from `0` toward
//!   `1` — [`LOADING_SPRING`] (`M3EMotion.expressiveSpatialDefault`,
//!   `m3e_motion.dart:104-107`, promoted to this crate's shared
//!   [`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`] token) released with
//!   [`MORPH_INITIAL_VELOCITY`] (`SpringSimulation(spring, 0, 1, 5,
//!   snapToEnd: true)`, `m3e_expressive_loading_indicator.dart:85-91`).
//! - **Continuous background rotation.** A separate `0.0..1.0` loop repeats
//!   every [`GLOBAL_ROTATION_PERIOD`] (4666ms — upstream's
//!   `_globalRotationDurationMs`), contributing up to a full turn.
//!
//! The paint reads both, plus a per-cycle accumulator
//! (`rotation_target_deg`, upstream's `_morphRotationTargetAngle`) that steps
//! by [`QUARTER_ROTATION_DEG`] (90°) modulo [`FULL_ROTATION_DEG`] every
//! cycle, and sums them exactly as upstream's `totalRotationDegrees` does —
//! so the shape visibly spins a quarter-turn per morph on top of the slow
//! continuous background spin.
//!
//! # Initial cycle
//!
//! Upstream's `initState` calls `_startMorphCycle()` once, synchronously,
//! before the first frame ever paints (`m3e_expressive_loading_indicator.dart:192,259`).
//! That call already advances `_currentMorphIndex` from `0` to `1` and
//! `_morphRotationTargetAngle` from `90°` to `180°` — so the very first shape
//! ever shown is [`SHAPE_CYCLE`]`[1]` (`cookie9Sided`), not `[0]`
//! (`softBurst`); `[0]` only reappears later in the loop. [`LoadingIndicatorWidget::advance_cycle`]
//! replays this by running once at the end of `build`, matching the quirk
//! exactly rather than "fixing" it.
//!
//! # Geometry
//!
//! Every catalog shape is normalized into `(0,0)..(1,1)`
//! ([`crate::shapes::RoundedPolygon::normalized`]), so a raw [`Morph::path`] output needs
//! scaling and centering before it paints. [`shape_cycle_scale_factor`] is
//! upstream's `_calculateScaleFactor`
//! (`m3e_expressive_loading_indicator.dart:213-247`): since the shape
//! rotates, sizing it to fit only its own (unrotated) bounds would clip a
//! corner as it turns, so the scale is instead derived from the *worst-case*
//! ratio between each shape's axis-aligned bounds and its maximum
//! any-rotation bounds ([`crate::shapes::RoundedPolygon::max_bounds`]) across the whole
//! cycle. [`process_path`] applies that scale (further scaled by
//! [`ACTIVE_INDICATOR_SIZE`] against the container box — upstream's
//! `activeIndicatorScale`), centers the result, and rotates it about that
//! same center — the upstream `_MorphPainter._processPath` scale+center
//! composed with the enclosing `Transform.rotate`
//! (`m3e_expressive_loading_indicator.dart:144-159`, `319-340`).
//!
//! # Variants
//!
//! [`LoadingIndicatorVariant::Default`] paints only the floating shape
//! (`containerColorDefault`, fully transparent — no background is drawn at
//! all). [`LoadingIndicatorVariant::Contained`] additionally fills a
//! fully-rounded container the same size as the indicator box behind it,
//! `colors.primary_container`/`colors.on_primary_container` in place of
//! `colors.primary` — upstream's `M3ELoadingIndicatorTheme`
//! (`m3e_loading_indicator_theme.dart`). The container's radius (`999`,
//! i.e. always fully rounded at this box size) and its zero default padding
//! mean the indicator's own geometry is unchanged between variants.
//!
//! # Dropped frames
//!
//! The per-cycle boundary is detected the same way the pre-rework module
//! detected it: a linear `0.0..1.0` repeat whose *wrap* (this frame's phase <
//! last frame's) triggers [`LoadingIndicatorWidget::advance_cycle`], rather
//! than a counted timer callback. A dropped frame longer than one full period
//! therefore skips a cycle visually rather than catching every intermediate
//! one — acceptable for a decorative loading spinner, and it always
//! self-corrects on the next frame.

use std::sync::OnceLock;
use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust::{AnimationController, Curve, FrameTime, SpringDesc, Theme};
use kurbo::{Affine, BezPath, Point, Shape, Size};
use peniko::{Brush, Color};

use crate::shapes::{Morph, ShapeKind};
use crate::tokens::MaterialSpring;

/// Overall diameter of the indicator/container, in logical px — upstream's
/// `M3ELoadingIndicatorTheme.containerWidth`/`containerHeight` defaults
/// (`m3e_loading_indicator_theme.dart:12-13`).
const LOADING_DIAMETER: f64 = 48.0;

/// Upstream's `_activeSize` — the shapes' target diameter within the
/// container box, "based on source spec"
/// (`m3e_expressive_loading_indicator.dart:74`).
const ACTIVE_INDICATOR_SIZE: f64 = 38.0;

/// Upstream's `_morphIntervalMs` — how long the indicator dwells on/morphs
/// through each shape pair before advancing to the next
/// (`m3e_expressive_loading_indicator.dart:70`).
const PER_SHAPE_PERIOD: Duration = Duration::from_millis(650);

/// Upstream's `_globalRotationDurationMs` — the period of the continuous
/// background rotation layered under the per-cycle morph rotation
/// (`m3e_expressive_loading_indicator.dart:69`).
const GLOBAL_ROTATION_PERIOD: Duration = Duration::from_millis(4666);

/// Upstream's `_quarterRotation` (`_fullRotation / 4`) — the rotation, in
/// degrees, a single morph cycle contributes: both the in-flight morph's own
/// sweep and the fixed step `_morphRotationTargetAngle` accumulates every
/// cycle (`m3e_expressive_loading_indicator.dart:71,73`).
const QUARTER_ROTATION_DEG: f64 = 90.0;

/// Upstream's `_fullRotation` (`m3e_expressive_loading_indicator.dart:71`).
const FULL_ROTATION_DEG: f64 = 360.0;

/// The morph spring's initial release velocity — upstream's
/// `SpringSimulation(spring, 0, 1, 5, snapToEnd: true)`
/// (`m3e_expressive_loading_indicator.dart:85-91`).
const MORPH_INITIAL_VELOCITY: f64 = 5.0;

/// The per-morph spring — `M3EMotion.expressiveSpatialDefault` (stiffness
/// `380`, damping ratio `0.8` — `m3e_motion.dart:104-107`), the same preset
/// [`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`] promotes to a shared crate
/// token; `m3e_expressive_loading_indicator.dart:85-91` names it directly.
/// Mass `1.0` (every M3 spring preset is mass-1; see `frust::MotionSpring`).
const LOADING_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.damping_ratio,
};

/// Unthemed indicator fill fallback for [`LoadingIndicatorVariant::Default`]
/// (shares [`super::progress`]'s unthemed primary; a theme resolves this
/// from `colors.primary`).
const FALLBACK_FILL: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);

/// Unthemed fallback for `colors.primary_container` — the
/// [`LoadingIndicatorVariant::Contained`] background (the baseline
/// light-scheme literal, `tokens/color.rs`).
const FALLBACK_PRIMARY_CONTAINER: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);

/// Unthemed fallback for `colors.on_primary_container` — the
/// [`LoadingIndicatorVariant::Contained`] shape fill (the baseline
/// light-scheme literal, `tokens/color.rs`).
const FALLBACK_ON_PRIMARY_CONTAINER: Color = Color::from_rgb8(0x21, 0x00, 0x5D);

/// The number of shapes the indicator cycles through — upstream's
/// `_defaultPolygons.length` (`m3e_expressive_loading_indicator.dart:57-65`).
const SHAPE_CYCLE_LEN: usize = 7;

/// The default shape sequence — see the [module docs](self)'s Shape sequence
/// section.
const SHAPE_CYCLE: [ShapeKind; SHAPE_CYCLE_LEN] = [
    ShapeKind::SoftBurst,
    ShapeKind::Cookie9Sided,
    ShapeKind::Pentagon,
    ShapeKind::Pill,
    ShapeKind::Sunny,
    ShapeKind::Cookie4Sided,
    ShapeKind::Oval,
];

/// The circular morph sequence over [`SHAPE_CYCLE`] — see the
/// [module docs](self)'s Shape sequence section. Built once and shared by
/// every [`LoadingIndicatorWidget`] instance.
fn morph_sequence() -> &'static [Morph; SHAPE_CYCLE_LEN] {
    static CACHE: OnceLock<[Morph; SHAPE_CYCLE_LEN]> = OnceLock::new();
    CACHE.get_or_init(|| {
        std::array::from_fn(|i| {
            let start = SHAPE_CYCLE[i].polygon().clone();
            let end = SHAPE_CYCLE[(i + 1) % SHAPE_CYCLE_LEN].polygon().clone();
            Morph::new(start, end)
        })
    })
}

/// The uniform scale applied to every morph's normalized outline so the
/// shape never clips its container while it rotates — see the
/// [module docs](self)'s Geometry section.
fn shape_cycle_scale_factor() -> f64 {
    static CACHE: OnceLock<f64> = OnceLock::new();
    *CACHE.get_or_init(|| {
        SHAPE_CYCLE.iter().fold(1.0_f64, |acc, kind| {
            let polygon = kind.polygon();
            let bounds = polygon.bounds();
            let max_bounds = polygon.max_bounds();
            let scale_x = bounds.width() / max_bounds.width();
            let scale_y = bounds.height() / max_bounds.height();
            acc.min(scale_x.max(scale_y))
        })
    })
}

/// Scales `path` (a [`Morph::path`] output, normalized into `(0,0)..(1,1)`)
/// by `size * scale_factor`, centers its bounds in `size`, and rotates it
/// about that same center by `rotation_rad` — see the [module docs](self)'s
/// Geometry section.
fn process_path(path: &BezPath, size: Size, scale_factor: f64, rotation_rad: f64) -> BezPath {
    let scale = Affine::new([
        size.width * scale_factor,
        0.0,
        0.0,
        size.height * scale_factor,
        0.0,
        0.0,
    ]);
    let scaled = scale * path;
    let bounds = scaled.bounding_box();
    let center = Point::new(size.width / 2.0, size.height / 2.0);
    let centered = Affine::translate(center - bounds.center()) * scaled;
    Affine::rotate_about(rotation_rad, center) * centered
}

/// The indicator's visual variant — upstream's `M3ELoadingIndicatorVariant`
/// (`m3e_loading_indicator_variant.dart`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadingIndicatorVariant {
    /// A floating morphing shape drawn directly on the surface — no
    /// container background is painted (upstream's `containerColorDefault`,
    /// fully transparent).
    Default,
    /// The morphing shape inside a filled, fully-rounded container —
    /// `colors.primary_container` background, `colors.on_primary_container`
    /// shape.
    Contained,
}

/// A Material 3 Expressive loading indicator. See the [module docs](self).
pub struct LoadingIndicatorView {
    variant: LoadingIndicatorVariant,
}

/// Create a loading indicator ([`LoadingIndicatorVariant::Default`]).
pub fn loading_indicator() -> LoadingIndicatorView {
    LoadingIndicatorView {
        variant: LoadingIndicatorVariant::Default,
    }
}

/// PascalCase alias for [`loading_indicator`], matching the widget-fn
/// vocabulary (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn LoadingIndicator() -> LoadingIndicatorView {
    loading_indicator()
}

impl LoadingIndicatorView {
    /// Switch to the [`LoadingIndicatorVariant::Contained`] variant: the
    /// shape paints inside a filled, fully-rounded container. See the
    /// [module docs](self)'s Variants section.
    pub fn contained(mut self) -> Self {
        self.variant = LoadingIndicatorVariant::Contained;
        self
    }
}

impl<State: 'static> View<State> for LoadingIndicatorView {
    type Element = LoadingIndicatorWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LoadingIndicatorWidget {
        let mut cycle_timer = AnimationController::new(PER_SHAPE_PERIOD).with_curve(Curve::Linear);
        cycle_timer.repeat();
        let mut rotation =
            AnimationController::new(GLOBAL_ROTATION_PERIOD).with_curve(Curve::Linear);
        rotation.repeat();

        let mut widget = LoadingIndicatorWidget {
            variant: self.variant,
            cycle_timer,
            last_cycle_phase: 0.0,
            morph_index: 0,
            morph: AnimationController::new(Duration::ZERO),
            rotation,
            rotation_target_deg: QUARTER_ROTATION_DEG,
        };
        // Upstream's `initState` calls `_startMorphCycle()` once, before the
        // first frame — see the module docs' Initial cycle section.
        widget.advance_cycle();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LoadingIndicatorWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.variant != self.variant {
            element.variant = self.variant;
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// The retained widget for a [`LoadingIndicatorView`]. See the [module docs](self).
pub struct LoadingIndicatorWidget {
    variant: LoadingIndicatorVariant,
    /// Linear 650ms-period repeat marking every shape-cycle boundary — the
    /// same wrap-detection idiom the pre-rework module used
    /// ([`Self::step`]), now advancing a [`Morph`] sequence instead of a raw
    /// radial interpolation.
    cycle_timer: AnimationController,
    /// Last frame's [`Self::cycle_timer`] phase, to detect a wrap.
    last_cycle_phase: f64,
    /// Index into [`morph_sequence`] of the morph currently in progress.
    morph_index: usize,
    /// The current morph's spring-driven progress — reset and re-flung every
    /// cycle (upstream's `_morphController`).
    morph: AnimationController,
    /// Continuous background rotation, one full turn per
    /// [`GLOBAL_ROTATION_PERIOD`] (upstream's `_globalRotationController`).
    rotation: AnimationController,
    /// Accumulated per-cycle rotation target, degrees (upstream's
    /// `_morphRotationTargetAngle`), advancing by [`QUARTER_ROTATION_DEG`]
    /// modulo [`FULL_ROTATION_DEG`] every cycle.
    rotation_target_deg: f64,
}

impl LoadingIndicatorWidget {
    /// Advances to the next morph in the cycle: bumps [`Self::morph_index`]
    /// and [`Self::rotation_target_deg`], and restarts [`Self::morph`] with a
    /// fresh spring fling — upstream's `_startMorphCycle`
    /// (`m3e_expressive_loading_indicator.dart:262-278`).
    fn advance_cycle(&mut self) {
        self.morph_index = (self.morph_index + 1) % SHAPE_CYCLE_LEN;
        self.rotation_target_deg =
            (self.rotation_target_deg + QUARTER_ROTATION_DEG) % FULL_ROTATION_DEG;
        self.morph = AnimationController::new(Duration::ZERO);
        self.morph.fling(MORPH_INITIAL_VELOCITY, LOADING_SPRING);
    }

    /// The indicator's active shape color and (for
    /// [`LoadingIndicatorVariant::Contained`]) its container background —
    /// upstream's `resolveActiveColor`/`resolveContainerColor`
    /// (`m3e_loading_indicator_theme.dart:53-75`). `None` for the default
    /// variant's fully-transparent `containerColorDefault`.
    fn resolve_colors(&self, theme: Option<&Theme>) -> (Color, Option<Color>) {
        let scheme = theme.map(Theme::scheme);
        match self.variant {
            LoadingIndicatorVariant::Default => {
                let active = scheme.map(|s| s.primary).unwrap_or(FALLBACK_FILL);
                (active, None)
            }
            LoadingIndicatorVariant::Contained => {
                let active = scheme
                    .map(|s| s.on_primary_container)
                    .unwrap_or(FALLBACK_ON_PRIMARY_CONTAINER);
                let container = scheme
                    .map(|s| s.primary_container)
                    .unwrap_or(FALLBACK_PRIMARY_CONTAINER);
                (active, Some(container))
            }
        }
    }

    /// Advance every clock to frame time `now` and, on a
    /// [`Self::cycle_timer`] wrap, start the next morph cycle (see the
    /// [module docs](self)'s Dropped frames section). Split out of `paint`
    /// so the wrap logic is drivable in a unit test without threading a
    /// shell clock through `PaintCtx` (whose frame-time setter is
    /// crate-private to `frust-core`).
    fn step(&mut self, now: FrameTime) {
        self.cycle_timer.advance(now);
        let phase = self.cycle_timer.value_clamped();
        if phase < self.last_cycle_phase {
            self.advance_cycle();
        }
        self.last_cycle_phase = phase;
        self.morph.advance(now);
        self.rotation.advance(now);
    }
}

impl Widget for LoadingIndicatorWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(LOADING_DIAMETER, LOADING_DIAMETER))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (active_color, container_color) = self.resolve_colors(theme);
        let size = ctx.size();

        // `reduce_motion` freezes the morph/rotation wherever they currently
        // sit and stops requesting frames — the same skip-animation shape
        // `glyph::skeleton`'s shimmer uses.
        if !reduce_motion {
            self.step(ctx.frame_time());
        }

        if let Some(container_color) = container_color {
            let radius = size.width.min(size.height) / 2.0;
            scene.fill_rounded_rect(ctx.origin(), size, radius, container_color);
        }

        let morph_progress = self.morph.value_clamped();
        let global_rotation_deg = self.rotation.value() * FULL_ROTATION_DEG;
        let total_rotation_deg =
            morph_progress * QUARTER_ROTATION_DEG + self.rotation_target_deg + global_rotation_deg;

        let morph = &morph_sequence()[self.morph_index];
        let raw_path = morph.path(morph_progress);
        let scale_factor =
            shape_cycle_scale_factor() * (ACTIVE_INDICATOR_SIZE / size.width.min(size.height));
        let path = process_path(
            &raw_path,
            size,
            scale_factor,
            total_rotation_deg.to_radians(),
        );

        scene.fill_path(ctx.origin(), &path, &Brush::Solid(active_color));

        if !reduce_motion {
            // A perpetually looping morph is a decorative loop — its exact
            // cadence is imperceptible, so the mobile frame gate may pace it.
            ctx.request_frame_paced();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use kurbo::PathEl;

    fn build() -> LoadingIndicatorWidget {
        let view = loading_indicator();
        let mut counter = 0u64;
        <LoadingIndicatorView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn build_contained() -> LoadingIndicatorWidget {
        let view = loading_indicator().contained();
        let mut counter = 0u64;
        <LoadingIndicatorView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[derive(Default)]
    struct RecordingScene {
        fills: Vec<BezPath>,
        rounded_rects: Vec<(Size, f64, Color)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_path(&mut self, _origin: Point, path: &BezPath, _brush: &Brush) {
            self.fills.push(path.clone());
        }
        fn fill_rounded_rect(&mut self, _o: Point, s: Size, radius: f64, color: Color) {
            self.rounded_rects.push((s, radius, color));
        }
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    // -----------------------------------------------------------------------
    // Constants transcribed from the reference (citations in each doc
    // comment above).
    // -----------------------------------------------------------------------

    #[test]
    fn shape_cycle_matches_the_reference_default_polygons() {
        // m3e_expressive_loading_indicator.dart:57-65.
        assert_eq!(
            SHAPE_CYCLE,
            [
                ShapeKind::SoftBurst,
                ShapeKind::Cookie9Sided,
                ShapeKind::Pentagon,
                ShapeKind::Pill,
                ShapeKind::Sunny,
                ShapeKind::Cookie4Sided,
                ShapeKind::Oval,
            ]
        );
    }

    #[test]
    fn timing_constants_match_the_reference() {
        assert_eq!(PER_SHAPE_PERIOD, Duration::from_millis(650)); // :70
        assert_eq!(GLOBAL_ROTATION_PERIOD, Duration::from_millis(4666)); // :69
        assert_eq!(QUARTER_ROTATION_DEG, 90.0); // :71,73
        assert_eq!(FULL_ROTATION_DEG, 360.0); // :71
        assert_eq!(ACTIVE_INDICATOR_SIZE, 38.0); // :74
        assert_eq!(LOADING_DIAMETER, 48.0); // m3e_loading_indicator_theme.dart:12-13
    }

    #[test]
    fn morph_initial_velocity_matches_the_reference() {
        // SpringSimulation(spring, 0, 1, 5, snapToEnd: true) —
        // m3e_expressive_loading_indicator.dart:85-91.
        assert_eq!(MORPH_INITIAL_VELOCITY, 5.0);
    }

    #[test]
    fn morph_spring_matches_expressive_spatial_default() {
        // m3e_motion.dart:104-107.
        assert_eq!(LOADING_SPRING.mass, 1.0);
        assert_eq!(LOADING_SPRING.stiffness, 380.0);
        assert_eq!(LOADING_SPRING.damping_ratio, 0.8);
        assert_eq!(
            LOADING_SPRING.stiffness,
            MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness
        );
        assert_eq!(
            LOADING_SPRING.damping_ratio,
            MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.damping_ratio
        );
    }

    // -----------------------------------------------------------------------
    // Morph sequence
    // -----------------------------------------------------------------------

    #[test]
    fn morph_sequence_is_built_once_and_wraps_the_cycle() {
        let a = morph_sequence();
        let b = morph_sequence();
        assert!(std::ptr::eq(a, b), "the shared cache must not rebuild");
        assert_eq!(a.len(), SHAPE_CYCLE_LEN);
        // The last morph wraps back to the first shape (circularSequence:
        // true).
        assert_eq!(*a[SHAPE_CYCLE_LEN - 1].end(), *SHAPE_CYCLE[0].polygon());
        assert_eq!(*a[0].start(), *SHAPE_CYCLE[0].polygon());
    }

    // -----------------------------------------------------------------------
    // Paint
    // -----------------------------------------------------------------------

    #[test]
    fn paints_a_filled_path_and_keeps_requesting_frames() {
        let mut w = build();
        for _ in 0..5 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LOADING_DIAMETER, LOADING_DIAMETER));
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert_eq!(scene.fills.len(), 1, "paints exactly one morphing shape");
            assert!(
                scene.rounded_rects.is_empty(),
                "the default variant paints no container background"
            );
            assert!(matches!(
                scene.fills[0].elements().last(),
                Some(PathEl::ClosePath)
            ));
            assert!(ctx.needs_frame(), "a loop must keep requesting frames");
            assert!(
                ctx.needs_frame_paced_only(),
                "the morph loop is a CosmeticLoop request — the frame gate must be able to pace it"
            );
        }
    }

    #[test]
    fn painted_shape_stays_within_the_container_box_and_is_centered() {
        let mut w = build();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LOADING_DIAMETER, LOADING_DIAMETER));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);

        let bounds = scene.fills[0].bounding_box();
        assert!(bounds.x0 >= -1.0 && bounds.y0 >= -1.0, "{bounds:?}");
        assert!(
            bounds.x1 <= LOADING_DIAMETER + 1.0 && bounds.y1 <= LOADING_DIAMETER + 1.0,
            "{bounds:?}"
        );
        let center = bounds.center();
        assert!((center.x - LOADING_DIAMETER / 2.0).abs() < 1.0);
        assert!((center.y - LOADING_DIAMETER / 2.0).abs() < 1.0);
    }

    #[test]
    fn reduce_motion_freezes_the_morph_and_stops_requesting_frames() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build();
        let frozen_index = w.morph_index;
        for _ in 0..3 {
            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LOADING_DIAMETER, LOADING_DIAMETER))
                .with_theme(&theme);
            let mut scene = RecordingScene::default();
            w.paint(&mut ctx, &mut scene);
            assert_eq!(scene.fills.len(), 1, "still paints a frozen shape");
            assert!(
                !ctx.needs_frame(),
                "reduce_motion must not request a continuation frame"
            );
        }
        assert_eq!(
            w.morph_index, frozen_index,
            "reduce_motion never advances the morph cycle"
        );
    }

    #[test]
    fn contained_variant_paints_a_rounded_container_behind_the_shape() {
        let mut w = build_contained();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(LOADING_DIAMETER, LOADING_DIAMETER));
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);

        assert_eq!(scene.fills.len(), 1, "still paints the morphing shape");
        assert_eq!(
            scene.rounded_rects.len(),
            1,
            "paints one container behind it"
        );
        let (size, radius, color) = scene.rounded_rects[0];
        assert_eq!(size, Size::new(LOADING_DIAMETER, LOADING_DIAMETER));
        assert_eq!(
            radius,
            LOADING_DIAMETER / 2.0,
            "fully rounded (999-radius spec)"
        );
        assert_eq!(color, FALLBACK_PRIMARY_CONTAINER);
    }

    // -----------------------------------------------------------------------
    // Cycle advance
    // -----------------------------------------------------------------------

    #[test]
    fn initial_build_starts_one_cycle_ahead_matching_the_reference() {
        // See the module docs' Initial cycle section:
        // m3e_expressive_loading_indicator.dart:192,259,262-278.
        let w = build();
        assert_eq!(w.morph_index, 1);
        assert_eq!(w.rotation_target_deg, 180.0);
        assert!(
            w.morph.is_animating(),
            "the initial cycle already flung the morph spring"
        );
    }

    #[test]
    fn cycle_wraps_after_one_period_and_advances_index_and_rotation_target() {
        let mut w = build();
        assert_eq!(w.morph_index, 1);
        w.step(ft_secs(0.0)); // seed the clock (zero delta)
        // Just before one full 650ms period: still on the first cycle.
        w.step(ft_secs(0.6));
        assert_eq!(w.morph_index, 1);
        assert_eq!(w.rotation_target_deg, 180.0);
        // Past one full period: the phase wrapped, advancing to the next
        // cycle.
        w.step(ft_secs(0.7));
        assert_eq!(w.morph_index, 2);
        assert_eq!(w.rotation_target_deg, 270.0);
    }

    #[test]
    fn morph_index_wraps_around_the_shape_cycle() {
        let mut w = build();
        w.morph_index = SHAPE_CYCLE_LEN - 1;
        w.rotation_target_deg = 360.0 - QUARTER_ROTATION_DEG;
        w.step(ft_secs(0.0)); // seed
        w.step(ft_secs(0.6));
        w.step(ft_secs(0.7)); // wrap from the last cycle -> back to 0
        assert_eq!(w.morph_index, 0);
        assert_eq!(w.rotation_target_deg, 0.0);
        assert!(w.morph_index < SHAPE_CYCLE_LEN);
    }
}
