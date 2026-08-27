// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/buttons/` tree is itself vendored from m3e_buttons
// (MIT, © 2026 Mudit Purohit) — `utils/m3e_button_gradient_fill.dart`,
// `utils/m3e_button_gradient_foreground.dart`,
// `utils/m3e_button_gradient_outline.dart`,
// `utils/m3e_button_gradient_overlay.dart`,
// `utils/m3e_button_gradient_surface.dart`, the gradient fields of
// `styles/m3e_button_decoration.dart`, and `res/m3e_button_constants.dart`'s
// `kDisabledBackgroundAlpha`. `GradientAlignment`'s `within_rect` and
// [`implied_stops`]'s stop math port Flutter's own `dart:ui`/`painting`
// `Alignment`/`LinearGradient`/`RadialGradient`/`SweepGradient` `createShader`
// formulas — framework source, not vendored anywhere in this repo, cited by
// public behavior rather than by file/line.

//! Gradient decoration layers for [`super::ButtonWidget`]'s
//! [`super::ButtonDecoration`] seam: a ported [`GradientButtonDecoration`]
//! implementation painting fill/overlay/outline/foreground gradients over (or
//! instead of) the core's solid layers.
//!
//! # Gradient classes ported (from `buttons/utils/`)
//!
//! | Reference | Here | Note |
//! |---|---|---|
//! | `m3eGradientFillLayer` / `m3eGradientBackgroundBuilder` (`m3e_button_gradient_fill.dart:6`/`:32`) | [`GradientButtonDecoration::paint_fill`](super::ButtonDecoration::paint_fill) | disabled dims to [`DISABLED_CONTAINER_OPACITY`] — see below |
//! | `m3eGradientForegroundLayer` / `m3eGradientForegroundBuilder` (`m3e_button_gradient_foreground.dart:4`/`:22`) | [`super::ButtonDecoration::foreground_brush`] | label only, not the icon — see that method's doc |
//! | `m3eGradientOutlineLayer` / `M3EGradientOutlinePainter` / `m3eOutlineWidth` (`m3e_button_gradient_outline.dart:4`/`:26`/`:74`) | [`GradientButtonDecoration::paint_outline`](super::ButtonDecoration::paint_outline) | strokes [`ButtonSurface::outline_width`] regardless of variant, matching upstream's unconditional apply |
//! | `m3eOverlayOpacityFor` / `m3eStatesForInteraction` / `m3eGradientOverlayLayer` / `m3eResolveGradientOverlay` / `m3eUsesGradientOverlay` (`m3e_button_gradient_overlay.dart:6`/`:23`/`:37`/`:68`/`:90`) | [`GradientStates::from_surface`] + [`GradientButtonDecoration::paint_overlay`](super::ButtonDecoration::paint_overlay) | opacity reuses [`InteractionState::resolve_opacity`], numerically identical to `m3eOverlayOpacityFor`'s own table |
//! | `m3eGradientSurfaceBuilder` (`m3e_button_gradient_surface.dart:11`) | the seam itself: [`super::ButtonDecoration`]'s fill→overlay→outline paint order | assembled here as three independent trait methods rather than one nested builder closure, since [`super::ButtonWidget::paint`] already calls the three in that order |
//! | `m3eGradientSpanRect` (`m3e_button_gradient_span.dart:6`) | **not ported** | maps a button's local rect into a *sibling*'s box via `GlobalKey`/`RenderBox.findRenderObject`/`localToGlobal` — [`super::ButtonSurface`] carries no cross-widget lookup, and this is `ButtonGroup`'s shared-gradient-span concern (a future task), not a single button's |
//!
//! Flutter's `LinearGradient`/`RadialGradient`/`SweepGradient` themselves are
//! stock `dart:ui` classes, not authored anywhere in `material_3_expressive`
//! — [`GradientSpec`] ports their `createShader` position/stop math (see
//! [`GradientAlignment::within_rect`] and [`implied_stops`]) since the button
//! utils above only ever consume a `Gradient`, never define one.
//!
//! # `PaintScene` gradient-support finding: **native**
//!
//! [`peniko::Brush::Gradient`] flows unmodified through
//! [`frust::authoring::PaintScene::fill_rounded_rect_brush`]/`fill_path`/
//! `stroke_path`/`draw_glyph_run` into a real
//! [`frust_scene::Command`](https://docs.rs/frust-scene) carrying the brush,
//! and the CPU render tier hands a `Brush::Gradient` straight to
//! `vello_cpu::PaintType::Gradient` (`crates/frust-render/src/cpu_tier.rs:184`)
//! — so this module paints real gradients, not a banded-solid approximation.
//! The one gap is [`frust::authoring::PaintScene::fill_rect_brush`] (the
//! *unrounded* rect variant), whose default falls back to a solid color for a
//! non-`SceneBuilder` recorder scene; this module never calls it — every fill
//! goes through `fill_rounded_rect_brush`, which the button core already uses
//! for its own solid container fill.
//!
//! # Scope narrowed from the reference (both documented, not silent)
//!
//! - **Foreground gradient tints the label, not the icon.** See
//!   [`super::ButtonDecoration::foreground_brush`]'s doc for why: a gradient
//!   brush painted directly on a glyph run is pixel-equivalent to upstream's
//!   `ShaderMask`+`BlendMode.srcIn`, but the icon is an opaque nested
//!   [`frust::authoring::ChildPod`] this seam cannot reach into.
//! - **No `GradientTransform`/focal-point radial/custom `TileMode`.** Every
//!   gradient in this repo's own `material_3_expressive` (including its
//!   playground examples, `example/lib/pages/playground/do/buttons_playground.dart:152`)
//!   uses plain `LinearGradient(colors: [...])` with implied stops and no
//!   transform or focal point — [`GradientSpec`] ports exactly that surface
//!   (colors, explicit stops, begin/end/center/angles) and always resolves
//!   [`peniko::Extend::Pad`] (`dart:ui`'s own `TileMode.clamp` default),
//!   rather than speculatively porting knobs nothing in this codebase
//!   exercises.

use std::rc::Rc;

use frust::authoring::PaintScene;
use kurbo::{Point, Shape, Size};
use peniko::{
    Brush, Color, ColorStop, Gradient, GradientKind, LinearGradientPosition,
    RadialGradientPosition, SweepGradientPosition,
};

use crate::interaction::DISABLED_CONTAINER_OPACITY;

use super::core::{PATH_TOLERANCE, inset_stroke_rect};
use super::{ButtonDecoration, ButtonSurface, DecorationOutcome};

// ---- Alignment + stop math (Flutter's own dart:ui Gradient math) ----------

/// A normalized `[-1, 1] x [-1, 1]` position, ported from `dart:ui`'s
/// `Alignment` — the coordinate space a Flutter `LinearGradient`'s
/// `begin`/`end` and a `RadialGradient`/`SweepGradient`'s `center` are
/// expressed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientAlignment {
    pub x: f64,
    pub y: f64,
}

impl GradientAlignment {
    /// `Alignment.centerLeft` — `LinearGradientSpec`'s default `begin`.
    pub const CENTER_LEFT: Self = Self { x: -1.0, y: 0.0 };
    /// `Alignment.centerRight` — `LinearGradientSpec`'s default `end`.
    pub const CENTER_RIGHT: Self = Self { x: 1.0, y: 0.0 };
    /// `Alignment.center` — `RadialGradientSpec`/`SweepGradientSpec`'s
    /// default `center`.
    pub const CENTER: Self = Self { x: 0.0, y: 0.0 };

    /// `Alignment.withinRect`: map this `[-1, 1]` position into the
    /// window-space box at `origin`/`size` — `x_px = origin.x + (1+x)/2 *
    /// width`, likewise for `y`. `origin` is window-space (matching
    /// [`ButtonSurface::origin`]) because a brush's own coordinates, unlike a
    /// `PaintScene` path, are never auto-translated by the scene's `origin`
    /// argument — see this module's `PaintScene` gradient-support finding.
    pub fn within_rect(&self, origin: Point, size: Size) -> Point {
        let half_w = size.width / 2.0;
        let half_h = size.height / 2.0;
        Point::new(
            origin.x + half_w + self.x * half_w,
            origin.y + half_h + self.y * half_h,
        )
    }
}

/// Evenly-spaced fallback stops for a gradient whose `stops` is unset —
/// `dart:ui`'s `Gradient._impliedStops()`: `n` colors get `n` stops at `i /
/// (n - 1)`; a single color pins to one stop at `0.0`; zero colors yields no
/// stops.
pub fn implied_stops(color_count: usize) -> Vec<f32> {
    match color_count {
        0 => Vec::new(),
        1 => vec![0.0],
        n => {
            let separation = 1.0 / (n - 1) as f32;
            (0..n).map(|i| i as f32 * separation).collect()
        }
    }
}

/// Zip `colors` with `stops` (or [`implied_stops`] when `stops` is `None`)
/// into the [`ColorStop`] list [`Gradient::with_stops`] takes.
fn color_stops(colors: &[Color], stops: &Option<Vec<f32>>) -> Vec<ColorStop> {
    let resolved = stops.clone().unwrap_or_else(|| implied_stops(colors.len()));
    colors
        .iter()
        .zip(resolved)
        .map(|(color, stop)| ColorStop::from((stop, *color)))
        .collect()
}

// ---- Gradient specs (the reference's `LinearGradient`/`RadialGradient`/
// ---- `SweepGradient`, as consumed by `WidgetStateProperty<Gradient?>`) ----

/// A linear gradient along the line from `begin` to `end` — `dart:ui`'s
/// `LinearGradient`.
#[derive(Clone, Debug, PartialEq)]
pub struct LinearGradientSpec {
    pub begin: GradientAlignment,
    pub end: GradientAlignment,
    pub colors: Vec<Color>,
    pub stops: Option<Vec<f32>>,
}

impl LinearGradientSpec {
    /// A gradient through `colors` with evenly-spaced stops, running left to
    /// right (`Alignment.centerLeft` to `Alignment.centerRight`) —
    /// `LinearGradient`'s own defaults.
    pub fn new(colors: impl Into<Vec<Color>>) -> Self {
        Self {
            begin: GradientAlignment::CENTER_LEFT,
            end: GradientAlignment::CENTER_RIGHT,
            colors: colors.into(),
            stops: None,
        }
    }

    /// Override the start alignment.
    pub fn begin(mut self, begin: GradientAlignment) -> Self {
        self.begin = begin;
        self
    }

    /// Override the end alignment.
    pub fn end(mut self, end: GradientAlignment) -> Self {
        self.end = end;
        self
    }

    /// Override the color stops (must match `colors`' length).
    pub fn stops(mut self, stops: Vec<f32>) -> Self {
        self.stops = Some(stops);
        self
    }

    fn resolve(&self, origin: Point, size: Size) -> Gradient {
        let start = self.begin.within_rect(origin, size);
        let end = self.end.within_rect(origin, size);
        Gradient::new_linear(start, end)
            .with_stops(color_stops(&self.colors, &self.stops).as_slice())
    }
}

/// A radial gradient centered at `center`, reaching `radius` (a fraction of
/// the paint box's shortest side) — `dart:ui`'s `RadialGradient`, minus its
/// `focal`/`focalRadius` two-point knobs (see this module's Scope Narrowed
/// section).
#[derive(Clone, Debug, PartialEq)]
pub struct RadialGradientSpec {
    pub center: GradientAlignment,
    /// Fraction of the box's shortest side, matching `RadialGradient.radius`
    /// (which is likewise a fraction, not an absolute length).
    pub radius: f64,
    pub colors: Vec<Color>,
    pub stops: Option<Vec<f32>>,
}

impl RadialGradientSpec {
    /// A gradient through `colors`, centered (`Alignment.center`) at radius
    /// `0.5` — `RadialGradient`'s own defaults.
    pub fn new(colors: impl Into<Vec<Color>>) -> Self {
        Self {
            center: GradientAlignment::CENTER,
            radius: 0.5,
            colors: colors.into(),
            stops: None,
        }
    }

    /// Override the center alignment.
    pub fn center(mut self, center: GradientAlignment) -> Self {
        self.center = center;
        self
    }

    /// Override the radius (a fraction of the shortest side).
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// Override the color stops (must match `colors`' length).
    pub fn stops(mut self, stops: Vec<f32>) -> Self {
        self.stops = Some(stops);
        self
    }

    fn resolve(&self, origin: Point, size: Size) -> Gradient {
        let center = self.center.within_rect(origin, size);
        // `RadialGradient.createShader`: `radius * rect.shortestSide`.
        let shortest_side = size.width.min(size.height);
        let radius = (self.radius * shortest_side) as f32;
        Gradient::new_radial(center, radius)
            .with_stops(color_stops(&self.colors, &self.stops).as_slice())
    }
}

/// A sweep (conic) gradient rotating around `center` from `start_angle` to
/// `end_angle`, in radians measured clockwise from the positive X-axis (a
/// Y-down coordinate system) — `dart:ui`'s `SweepGradient`, and the exact
/// convention [`peniko::SweepGradientPosition`] itself documents, so no
/// angle-flip is needed at the boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepGradientSpec {
    pub center: GradientAlignment,
    pub start_angle: f64,
    pub end_angle: f64,
    pub colors: Vec<Color>,
    pub stops: Option<Vec<f32>>,
}

impl SweepGradientSpec {
    /// A gradient through `colors`, centered (`Alignment.center`) sweeping a
    /// full turn (`0` to `2π`) — `SweepGradient`'s own defaults.
    pub fn new(colors: impl Into<Vec<Color>>) -> Self {
        Self {
            center: GradientAlignment::CENTER,
            start_angle: 0.0,
            end_angle: std::f64::consts::TAU,
            colors: colors.into(),
            stops: None,
        }
    }

    /// Override the center alignment.
    pub fn center(mut self, center: GradientAlignment) -> Self {
        self.center = center;
        self
    }

    /// Override the start/end sweep angles, in radians.
    pub fn angles(mut self, start_angle: f64, end_angle: f64) -> Self {
        self.start_angle = start_angle;
        self.end_angle = end_angle;
        self
    }

    /// Override the color stops (must match `colors`' length).
    pub fn stops(mut self, stops: Vec<f32>) -> Self {
        self.stops = Some(stops);
        self
    }

    fn resolve(&self, origin: Point, size: Size) -> Gradient {
        let center = self.center.within_rect(origin, size);
        Gradient::new_sweep(center, self.start_angle as f32, self.end_angle as f32)
            .with_stops(color_stops(&self.colors, &self.stops).as_slice())
    }
}

/// The reference's `Gradient` sealed hierarchy as consumed by a button
/// decoration — one of [`LinearGradientSpec`]/[`RadialGradientSpec`]/
/// [`SweepGradientSpec`].
#[derive(Clone, Debug, PartialEq)]
pub enum GradientSpec {
    Linear(LinearGradientSpec),
    Radial(RadialGradientSpec),
    Sweep(SweepGradientSpec),
}

impl GradientSpec {
    /// Resolve to a real [`peniko::Gradient`] against the window-space box at
    /// `origin`/`size` — the reference's `Gradient.createShader(Rect rect)`.
    pub fn resolve(&self, origin: Point, size: Size) -> Gradient {
        match self {
            Self::Linear(spec) => spec.resolve(origin, size),
            Self::Radial(spec) => spec.resolve(origin, size),
            Self::Sweep(spec) => spec.resolve(origin, size),
        }
    }
}

impl From<LinearGradientSpec> for GradientSpec {
    fn from(spec: LinearGradientSpec) -> Self {
        Self::Linear(spec)
    }
}

impl From<RadialGradientSpec> for GradientSpec {
    fn from(spec: RadialGradientSpec) -> Self {
        Self::Radial(spec)
    }
}

impl From<SweepGradientSpec> for GradientSpec {
    fn from(spec: SweepGradientSpec) -> Self {
        Self::Sweep(spec)
    }
}

// ---- Per-state resolution (m3eStatesForInteraction) ------------------------

/// The `Set<WidgetState>` snapshot a [`GradientProperty`] resolves against —
/// `m3eStatesForInteraction` (`m3e_button_gradient_overlay.dart:23`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GradientStates {
    pub hovered: bool,
    pub focused: bool,
    pub pressed: bool,
    pub dragged: bool,
    pub disabled: bool,
}

impl GradientStates {
    /// Derive the states a [`ButtonSurface`]'s live interaction state and
    /// `enabled` flag represent.
    pub fn from_surface(surface: &ButtonSurface) -> Self {
        Self {
            hovered: surface.state.hovered,
            focused: surface.state.focused,
            pressed: surface.state.pressed,
            dragged: surface.state.dragged,
            disabled: !surface.enabled,
        }
    }
}

/// A per-state gradient resolver — the reference's `WidgetStateProperty<Gradient?>`.
pub type GradientProperty = Rc<dyn Fn(GradientStates) -> Option<GradientSpec>>;

/// A [`GradientProperty`] that resolves to `spec` in every state —
/// `WidgetStateProperty.all(gradient)`, the shape every gradient button in
/// `material_3_expressive`'s own playground uses
/// (`example/lib/pages/playground/do/buttons_playground.dart:151`).
pub fn constant_gradient(spec: impl Into<GradientSpec>) -> GradientProperty {
    let spec = spec.into();
    Rc::new(move |_states| Some(spec.clone()))
}

// ---- The decoration ---------------------------------------------------------

/// A [`ButtonDecoration`] painting gradient fill/overlay/outline/foreground
/// layers — the reference's `M3EButtonDecoration` gradient quadruple
/// (`backgroundGradient`/`overlayGradient`/`outlineGradient`/
/// `foregroundGradient`, `m3e_button_decoration.dart:83`-`:92`) realized
/// against this crate's [`ButtonDecoration`] seam. See the [module docs](self)
/// for the full gradient-class inventory and scope notes.
#[derive(Clone, Default)]
pub struct GradientButtonDecoration {
    background: Option<GradientProperty>,
    foreground: Option<GradientProperty>,
    overlay: Option<GradientProperty>,
    outline: Option<GradientProperty>,
}

impl GradientButtonDecoration {
    /// A decoration with no layer configured — every hook falls through to
    /// [`DecorationOutcome::Skipped`]/`None` until a layer is set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the container fill gradient (`backgroundGradient`).
    pub fn background(mut self, property: GradientProperty) -> Self {
        self.background = Some(property);
        self
    }

    /// Set the label ink gradient (`foregroundGradient`). See
    /// [`ButtonDecoration::foreground_brush`] for the label-only scope note.
    pub fn foreground(mut self, property: GradientProperty) -> Self {
        self.foreground = Some(property);
        self
    }

    /// Set the interaction state-layer gradient (`overlayGradient`).
    pub fn overlay(mut self, property: GradientProperty) -> Self {
        self.overlay = Some(property);
        self
    }

    /// Set the hairline gradient (`outlineGradient`).
    pub fn outline(mut self, property: GradientProperty) -> Self {
        self.outline = Some(property);
        self
    }
}

impl ButtonDecoration for GradientButtonDecoration {
    fn paint_fill(&self, surface: &ButtonSurface, scene: &mut dyn PaintScene) -> DecorationOutcome {
        let Some(property) = &self.background else {
            return DecorationOutcome::Skipped;
        };
        let Some(spec) = property(GradientStates::from_surface(surface)) else {
            return DecorationOutcome::Skipped;
        };
        let mut gradient = spec.resolve(surface.origin, surface.size);
        if !surface.enabled {
            // `m3eGradientFillLayer`'s `disabled` `Opacity` wrap
            // (`m3e_button_gradient_fill.dart:19`) at
            // `M3EButtonConstants.kDisabledBackgroundAlpha` (12%,
            // `m3e_button_constants.dart:30`) — numerically the same 12% as
            // this crate's own [`DISABLED_CONTAINER_OPACITY`], reused rather
            // than re-declared.
            gradient = gradient.multiply_alpha(DISABLED_CONTAINER_OPACITY);
        }
        scene.fill_rounded_rect_brush(
            surface.origin,
            surface.size,
            surface.radius,
            &Brush::Gradient(gradient),
        );
        DecorationOutcome::Painted
    }

    fn paint_overlay(
        &self,
        surface: &ButtonSurface,
        scene: &mut dyn PaintScene,
    ) -> DecorationOutcome {
        let Some(property) = &self.overlay else {
            return DecorationOutcome::Skipped;
        };
        // `m3eUsesGradientOverlay` (`m3e_button_gradient_overlay.dart:90`)
        // checks only that the *property* is configured, not what it
        // resolves to for the current state — configuring one unconditionally
        // suppresses the core's own state layer, matching upstream's
        // `overlayColor: transparent` override (`m3e_button_style.dart:47`).
        if let Some(spec) = property(GradientStates::from_surface(surface)) {
            // `m3eOverlayOpacityFor` (`m3e_button_gradient_overlay.dart:6`):
            // disabled -> 0, else the precedence table
            // `InteractionState::resolve_opacity` already ports.
            let opacity = if surface.enabled {
                surface.state.resolve_opacity()
            } else {
                0.0
            };
            if opacity > 0.0 {
                let gradient = spec
                    .resolve(surface.origin, surface.size)
                    .multiply_alpha(opacity);
                scene.fill_rounded_rect_brush(
                    surface.origin,
                    surface.size,
                    surface.radius,
                    &Brush::Gradient(gradient),
                );
            }
        }
        DecorationOutcome::Painted
    }

    fn paint_outline(
        &self,
        surface: &ButtonSurface,
        scene: &mut dyn PaintScene,
    ) -> DecorationOutcome {
        let Some(property) = &self.outline else {
            return DecorationOutcome::Skipped;
        };
        let Some(spec) = property(GradientStates::from_surface(surface)) else {
            return DecorationOutcome::Skipped;
        };
        if surface.outline_width <= 0.0 {
            // `m3eGradientOutlineLayer`: `if (width <= 0 ...) return child;`
            // (`m3e_button_gradient_outline.dart:11`).
            return DecorationOutcome::Skipped;
        }
        let gradient = spec.resolve(surface.origin, surface.size);
        let path = inset_stroke_rect(surface.size, surface.radius, surface.outline_width)
            .to_path(PATH_TOLERANCE);
        scene.stroke_path(
            surface.origin,
            &path,
            surface.outline_width,
            &Brush::Gradient(gradient),
        );
        DecorationOutcome::Painted
    }

    fn foreground_brush(&self, surface: &ButtonSurface) -> Option<Brush> {
        let property = self.foreground.as_ref()?;
        let spec = property(GradientStates::from_surface(surface))?;
        Some(Brush::Gradient(spec.resolve(surface.origin, surface.size)))
    }
}

/// Re-expresses `brush`'s gradient geometry (if any) as an offset from
/// `run_origin` rather than the window origin it was resolved against —
/// [`ButtonDecoration::foreground_brush`]'s return value is window-space,
/// matching every other [`ButtonSurface`]-anchored brush (see
/// [`GradientAlignment::within_rect`]'s doc), but a
/// [`frust::authoring::scene::GlyphRun`]'s `brush` is painted under the
/// run's OWN `transform` — unlike [`PaintScene::fill_rounded_rect_brush`]/
/// `stroke_path`, whose separate `origin` argument never auto-translates
/// their brush (see this module's `PaintScene` gradient-support finding),
/// the glyph-run render path applies `transform` to the active paint too
/// (vello's `Scene::draw_glyphs(..).transform(run.transform).brush(..)` and
/// `vello_cpu`'s `set_transform` before `set_paint`, both in
/// `crates/frust-render`). Calling this with the exact origin the run's
/// `transform` will translate from — [`super::ButtonWidget`]'s paint pass's
/// `label_origin` — cancels that re-application, so the label's gradient
/// lands at the same window position [`ButtonDecoration::foreground_brush`]
/// resolved, regardless of where the label sits within the button. A
/// non-gradient brush (nothing to shift) is returned unchanged.
pub(super) fn brush_relative_to_run_origin(brush: &Brush, run_origin: Point) -> Brush {
    let Brush::Gradient(gradient) = brush else {
        return brush.clone();
    };
    let shift = |p: Point| Point::new(p.x - run_origin.x, p.y - run_origin.y);
    let mut gradient = gradient.clone();
    gradient.kind = match gradient.kind {
        GradientKind::Linear(pos) => LinearGradientPosition {
            start: shift(pos.start),
            end: shift(pos.end),
        }
        .into(),
        GradientKind::Radial(pos) => RadialGradientPosition {
            start_center: shift(pos.start_center),
            start_radius: pos.start_radius,
            end_center: shift(pos.end_center),
            end_radius: pos.end_radius,
        }
        .into(),
        GradientKind::Sweep(pos) => SweepGradientPosition {
            center: shift(pos.center),
            start_angle: pos.start_angle,
            end_angle: pos.end_angle,
        }
        .into(),
    };
    Brush::Gradient(gradient)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::{ButtonView, ButtonWidget, button};
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, View, Widget};
    use kurbo::BezPath;
    use peniko::{Extend, GradientKind};
    use std::any::Any;

    fn colors(hexes: &[u32]) -> Vec<Color> {
        hexes
            .iter()
            .map(|&h| {
                Color::from_rgb8(
                    ((h >> 16) & 0xFF) as u8,
                    ((h >> 8) & 0xFF) as u8,
                    (h & 0xFF) as u8,
                )
            })
            .collect()
    }

    // ---- implied_stops ------------------------------------------------

    #[test]
    fn implied_stops_matches_dart_uis_own_table() {
        assert_eq!(implied_stops(0), Vec::<f32>::new());
        assert_eq!(implied_stops(1), vec![0.0]);
        assert_eq!(implied_stops(2), vec![0.0, 1.0]);
        assert_eq!(implied_stops(3), vec![0.0, 0.5, 1.0]);
        assert_eq!(implied_stops(4), vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
    }

    // ---- gradient class 1: Linear, x2 sizes ----------------------------

    #[test]
    fn linear_gradient_resolves_begin_end_and_implied_stops_at_two_sizes() {
        let spec = LinearGradientSpec::new(colors(&[0x6750A4, 0x9A82DB]));
        for (origin, size) in [
            (Point::new(0.0, 0.0), Size::new(100.0, 40.0)),
            (Point::new(10.0, 20.0), Size::new(200.0, 80.0)),
        ] {
            let gradient = spec.resolve(origin, size);
            let GradientKind::Linear(pos) = gradient.kind else {
                panic!("expected a linear gradient kind");
            };
            // centerLeft -> (origin.x, origin.y + size.height / 2);
            // centerRight -> (origin.x + size.width, same y).
            assert_eq!(
                pos.start,
                Point::new(origin.x, origin.y + size.height / 2.0)
            );
            assert_eq!(
                pos.end,
                Point::new(origin.x + size.width, origin.y + size.height / 2.0)
            );
            assert_eq!(gradient.extend, Extend::Pad);
            let stops: Vec<(f32, Color)> = gradient
                .stops
                .iter()
                .map(|s| (s.offset, s.color.to_alpha_color()))
                .collect();
            assert_eq!(
                stops,
                vec![(0.0, colors(&[0x6750A4])[0]), (1.0, colors(&[0x9A82DB])[0]),]
            );
        }
    }

    #[test]
    fn linear_gradient_explicit_begin_end_and_stops_are_honored() {
        let spec = LinearGradientSpec::new(colors(&[0xFF0000, 0x00FF00, 0x0000FF]))
            .begin(GradientAlignment { x: 0.0, y: -1.0 })
            .end(GradientAlignment { x: 0.0, y: 1.0 })
            .stops(vec![0.0, 0.25, 1.0]);
        let origin = Point::new(0.0, 0.0);
        let size = Size::new(64.0, 32.0);
        let gradient = spec.resolve(origin, size);
        let GradientKind::Linear(pos) = gradient.kind else {
            panic!("expected a linear gradient kind");
        };
        // topCenter -> (origin.x + width/2, origin.y); bottomCenter -> (.., origin.y + height).
        assert_eq!(pos.start, Point::new(32.0, 0.0));
        assert_eq!(pos.end, Point::new(32.0, 32.0));
        let offsets: Vec<f32> = gradient.stops.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0.0, 0.25, 1.0]);
    }

    // ---- gradient class 2: Radial, x2 sizes ----------------------------

    #[test]
    fn radial_gradient_resolves_center_and_shortest_side_radius_at_two_sizes() {
        let spec = RadialGradientSpec::new(colors(&[0xFFFFFF, 0x000000]));
        for (origin, size) in [
            (Point::new(0.0, 0.0), Size::new(100.0, 40.0)),
            (Point::new(5.0, 5.0), Size::new(60.0, 90.0)),
        ] {
            let gradient = spec.resolve(origin, size);
            let GradientKind::Radial(pos) = gradient.kind else {
                panic!("expected a radial gradient kind");
            };
            let expected_center =
                Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
            assert_eq!(pos.start_center, expected_center);
            assert_eq!(pos.end_center, expected_center);
            assert_eq!(pos.start_radius, 0.0);
            let shortest_side = size.width.min(size.height);
            assert_eq!(pos.end_radius, (0.5 * shortest_side) as f32);
        }
    }

    #[test]
    fn radial_gradient_radius_override_scales_with_shortest_side() {
        let spec = RadialGradientSpec::new(colors(&[0xFFFFFF, 0x000000])).radius(1.0);
        let gradient = spec.resolve(Point::new(0.0, 0.0), Size::new(50.0, 200.0));
        let GradientKind::Radial(pos) = gradient.kind else {
            panic!("expected a radial gradient kind");
        };
        assert_eq!(pos.end_radius, 50.0, "radius scales off the shortest side");
    }

    // ---- gradient class 3: Sweep, x2 sizes -----------------------------

    #[test]
    fn sweep_gradient_defaults_to_a_full_clockwise_turn_at_two_sizes() {
        let spec = SweepGradientSpec::new(colors(&[0xFF0000, 0x00FF00, 0x0000FF]));
        for (origin, size) in [
            (Point::new(0.0, 0.0), Size::new(80.0, 80.0)),
            (Point::new(3.0, 4.0), Size::new(120.0, 40.0)),
        ] {
            let gradient = spec.resolve(origin, size);
            let GradientKind::Sweep(pos) = gradient.kind else {
                panic!("expected a sweep gradient kind");
            };
            assert_eq!(
                pos.center,
                Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
            );
            assert_eq!(pos.start_angle, 0.0);
            assert_eq!(pos.end_angle, std::f32::consts::TAU);
            assert_eq!(gradient.stops.len(), 3);
        }
    }

    // ---- GradientButtonDecoration: recording scene ----------------------

    #[derive(Default)]
    struct Recorder {
        rrect_brushes: Vec<Brush>,
        rrects: Vec<(Point, Size, f64, Color)>,
        stroke_brushes: Vec<Brush>,
        strokes: Vec<f64>,
        runs: Vec<Brush>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_rounded_rect_brush(&mut self, _o: Point, _s: Size, _radius: f64, brush: &Brush) {
            self.rrect_brushes.push(brush.clone());
        }
        fn stroke_path(&mut self, _o: Point, _path: &BezPath, width: f64, brush: &Brush) {
            self.stroke_brushes.push(brush.clone());
            self.strokes.push(width);
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.runs.push(run.brush);
        }
    }

    fn surface(state: InteractionStateForTest) -> ButtonSurface {
        use crate::interaction::InteractionState;
        ButtonSurface {
            origin: Point::new(4.0, 8.0),
            size: Size::new(120.0, 40.0),
            radius: 20.0,
            container: Color::TRANSPARENT,
            content: Color::from_rgb8(0, 0, 0),
            outline: None,
            outline_width: 1.0,
            enabled: state.enabled,
            state: InteractionState {
                hovered: state.hovered,
                focused: false,
                pressed: false,
                dragged: false,
            },
        }
    }

    struct InteractionStateForTest {
        enabled: bool,
        hovered: bool,
    }

    fn enabled_idle() -> InteractionStateForTest {
        InteractionStateForTest {
            enabled: true,
            hovered: false,
        }
    }

    fn enabled_hovered() -> InteractionStateForTest {
        InteractionStateForTest {
            enabled: true,
            hovered: true,
        }
    }

    fn disabled() -> InteractionStateForTest {
        InteractionStateForTest {
            enabled: false,
            hovered: false,
        }
    }

    fn linear_two_stop() -> GradientProperty {
        constant_gradient(LinearGradientSpec::new(colors(&[0x6750A4, 0x9A82DB])))
    }

    #[test]
    fn unconfigured_layers_skip_and_paint_nothing() {
        let deco = GradientButtonDecoration::new();
        let surf = surface(enabled_idle());
        let mut rec = Recorder::default();
        assert_eq!(deco.paint_fill(&surf, &mut rec), DecorationOutcome::Skipped);
        assert_eq!(
            deco.paint_overlay(&surf, &mut rec),
            DecorationOutcome::Skipped
        );
        assert_eq!(
            deco.paint_outline(&surf, &mut rec),
            DecorationOutcome::Skipped
        );
        assert!(deco.foreground_brush(&surf).is_none());
        assert!(rec.rrect_brushes.is_empty());
        assert!(rec.stroke_brushes.is_empty());
    }

    #[test]
    fn fill_gradient_paints_and_reports_painted_suppressing_the_core_fill() {
        let deco = GradientButtonDecoration::new().background(linear_two_stop());
        let surf = surface(enabled_idle());
        let mut rec = Recorder::default();
        assert_eq!(deco.paint_fill(&surf, &mut rec), DecorationOutcome::Painted);
        assert_eq!(rec.rrect_brushes.len(), 1);
        assert!(matches!(rec.rrect_brushes[0], Brush::Gradient(_)));
    }

    #[test]
    fn disabled_fill_gradient_dims_to_disabled_container_opacity() {
        let deco = GradientButtonDecoration::new().background(linear_two_stop());
        let surf = surface(disabled());
        let mut rec = Recorder::default();
        deco.paint_fill(&surf, &mut rec);
        let Brush::Gradient(gradient) = &rec.rrect_brushes[0] else {
            panic!("expected a gradient brush");
        };
        for stop in gradient.stops.iter() {
            assert_eq!(stop.color.components[3], DISABLED_CONTAINER_OPACITY);
        }
    }

    #[test]
    fn overlay_gradient_reports_painted_even_at_zero_opacity_but_paints_nothing() {
        // `m3eUsesGradientOverlay` unconditionally suppresses the core's own
        // state layer whenever the property is configured, independent of
        // whether *this* state resolves visible opacity.
        let deco = GradientButtonDecoration::new().overlay(linear_two_stop());
        let surf = surface(enabled_idle());
        let mut rec = Recorder::default();
        assert_eq!(
            deco.paint_overlay(&surf, &mut rec),
            DecorationOutcome::Painted
        );
        assert!(
            rec.rrect_brushes.is_empty(),
            "zero opacity paints no overlay quad"
        );
    }

    #[test]
    fn overlay_gradient_paints_at_the_hovered_states_opacity() {
        let deco = GradientButtonDecoration::new().overlay(linear_two_stop());
        let surf = surface(enabled_hovered());
        let mut rec = Recorder::default();
        deco.paint_overlay(&surf, &mut rec);
        assert_eq!(rec.rrect_brushes.len(), 1);
        let Brush::Gradient(gradient) = &rec.rrect_brushes[0] else {
            panic!("expected a gradient brush");
        };
        for stop in gradient.stops.iter() {
            assert_eq!(stop.color.components[3], crate::interaction::HOVER_OPACITY);
        }
    }

    #[test]
    fn disabled_overlay_gradient_paints_nothing_but_still_suppresses_the_core_layer() {
        let deco = GradientButtonDecoration::new().overlay(linear_two_stop());
        let surf = surface(disabled());
        let mut rec = Recorder::default();
        assert_eq!(
            deco.paint_overlay(&surf, &mut rec),
            DecorationOutcome::Painted
        );
        assert!(rec.rrect_brushes.is_empty());
    }

    #[test]
    fn outline_gradient_strokes_at_the_surfaces_outline_width() {
        let deco = GradientButtonDecoration::new().outline(linear_two_stop());
        let surf = surface(enabled_idle());
        let mut rec = Recorder::default();
        assert_eq!(
            deco.paint_outline(&surf, &mut rec),
            DecorationOutcome::Painted
        );
        assert_eq!(rec.strokes, vec![surf.outline_width]);
        assert!(matches!(rec.stroke_brushes[0], Brush::Gradient(_)));
    }

    #[test]
    fn foreground_brush_resolves_a_gradient_brush_when_configured() {
        let deco = GradientButtonDecoration::new().foreground(linear_two_stop());
        let surf = surface(enabled_idle());
        let brush = deco.foreground_brush(&surf);
        assert!(matches!(brush, Some(Brush::Gradient(_))));
    }

    // ---- coordinate-space regression: gradient tracks the glyph run -----

    /// A [`PaintScene`] that captures the *whole* [`GlyphRun`] (transform,
    /// glyphs, brush) rather than just its brush — needed to reconstruct
    /// what the render backend would actually draw (see
    /// [`brush_relative_to_run_origin`]'s doc).
    #[derive(Default)]
    struct GlyphRunRecorder {
        runs: Vec<GlyphRun>,
    }
    impl PaintScene for GlyphRunRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _radius: f64, _color: Color) {}
        fn fill_rounded_rect_brush(&mut self, _o: Point, _s: Size, _radius: f64, _brush: &Brush) {}
        fn stroke_path(&mut self, _o: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.runs.push(run);
        }
    }

    /// What the render backend actually paints a linear gradient's `start`
    /// at: `run.transform * gradient.start` — vello's
    /// `Scene::draw_glyphs(..).transform(run.transform).brush(&run.brush)`
    /// and `vello_cpu`'s `set_transform(run.transform)` before `set_paint`
    /// both apply the run's transform to its active paint, not just its
    /// glyph outlines (`crates/frust-render/src/convert.rs`/`cpu_tier.rs`).
    fn rendered_linear_start(run: &GlyphRun) -> Point {
        let Brush::Gradient(gradient) = &run.brush else {
            panic!("expected a gradient brush");
        };
        let GradientKind::Linear(pos) = gradient.kind else {
            panic!("expected a linear gradient kind");
        };
        run.transform * pos.start
    }

    /// The label's glyph run and its foreground gradient must stay locked
    /// together across consecutive paints at different origins (simulating
    /// a sliding page transition): both move by exactly the origin delta,
    /// and the gradient sits at the same offset from the glyph run in every
    /// frame — never a growing/shrinking drift.
    #[test]
    fn gradient_label_tracks_the_glyph_run_across_consecutive_paints_at_different_origins() {
        let deco: Rc<dyn ButtonDecoration> =
            Rc::new(GradientButtonDecoration::new().foreground(linear_two_stop()));
        let view = button::<(), _>("Save", |_| {}).decoration(deco);
        let mut widget = build(&view);

        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        let origin_delta = 10.0;
        let mut rec1 = GlyphRunRecorder::default();
        widget.paint(&mut PaintCtx::new(Point::new(10.0, 0.0), size), &mut rec1);
        let mut rec2 = GlyphRunRecorder::default();
        widget.paint(
            &mut PaintCtx::new(Point::new(10.0 + origin_delta, 0.0), size),
            &mut rec2,
        );

        let run1 = &rec1.runs[0];
        let run2 = &rec2.runs[0];
        let glyph1 = run1.transform * Point::new(run1.glyphs[0].x as f64, run1.glyphs[0].y as f64);
        let glyph2 = run2.transform * Point::new(run2.glyphs[0].x as f64, run2.glyphs[0].y as f64);
        let grad1 = rendered_linear_start(run1);
        let grad2 = rendered_linear_start(run2);

        assert_eq!(
            glyph2.x - glyph1.x,
            origin_delta,
            "glyph run must move by exactly the origin delta"
        );
        assert_eq!(
            grad2.x - grad1.x,
            origin_delta,
            "gradient must move by exactly the origin delta too, not a doubled (or otherwise \
             mismatched) amount a coordinate-space bug between the brush and the run's own \
             transform would produce"
        );
        assert_eq!(
            grad1.x - glyph1.x,
            grad2.x - glyph2.x,
            "the gradient's offset from its own glyph run must be the same constant in both \
             frames, never one that drifts with the button's position"
        );
    }

    // ---- end-to-end: the seam actually wired through ButtonWidget -------

    fn build<S: 'static>(view: &ButtonView<S>) -> ButtonWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn a_full_gradient_decoration_suppresses_every_core_solid_layer_and_paints_gradients() {
        let deco: Rc<dyn ButtonDecoration> = Rc::new(
            GradientButtonDecoration::new()
                .background(linear_two_stop())
                .outline(linear_two_stop())
                .foreground(linear_two_stop()),
        );
        let view = button::<(), _>("Save", |_| {}).decoration(deco);
        let mut widget = build(&view);

        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        widget.paint(&mut pctx, &mut rec);

        assert!(
            rec.rrects.is_empty(),
            "the solid fill must be suppressed by the gradient decoration"
        );
        assert!(
            !rec.rrect_brushes.is_empty(),
            "a gradient fill must have painted instead"
        );
        assert!(
            rec.stroke_brushes
                .iter()
                .any(|b| matches!(b, Brush::Gradient(_))),
            "the outline must have painted as a gradient stroke"
        );
        assert!(
            rec.runs.iter().any(|b| matches!(b, Brush::Gradient(_))),
            "the label run must have been re-brushed with the foreground gradient"
        );
    }
}
