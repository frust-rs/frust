//! `glass_surface(child)`: a baseline widget that paints one tier of the
//! active theme's [`GlassScale`] behind its child, with a real backdrop blur.
//!
//! The glass tokens ([`GlassMaterial`]) are pure data; this widget is the
//! single place that turns one into pixels, so a design system does not have
//! to hand-roll the same shadow, blur, wash and hairline sequence.
//!
//! # Sizing and semantics
//!
//! The surface is exactly the size of its child (pass-through layout), and
//! its semantics forward the child's. It is static and never requests a frame.
//!
//! # Paint order
//!
//! 1. The tier's drop shadow ([`PaintScene::draw_shadow`]), when its alpha is
//!    above zero, inside a clip that excludes the surface's own rounded
//!    rectangle (an outer rect with the surface as a hole), so the shadow is
//!    cast outside the surface and is never painted under the lens or the
//!    washes. The lens's blur footprint reaches past the surface edge, so it
//!    still samples the shadow ring just outside it. The exclusion needs a
//!    scene that clips to a path ([`PaintScene::push_clip_path`]'s default
//!    clips to the path's bounding box); a shadow margin too large to
//!    represent skips the clip and draws the shadow as the engine accepts it.
//! 2. A rounded clip at [`GlassSurfaceView::corner_radius`].
//! 3. Translucent material: [`PaintScene::backdrop_blur`] with the tier's
//!    `blur_radius_intent`, then the brightness-selected fill stack
//!    (`fills_light` or `fills_dark`) bottom-to-top. Opaque material: a single
//!    fill of the tier's elevation surface role instead (no blur). No theme
//!    and no explicit material: a single opaque fill of `#EFEFEF` (no blur).
//! 4. The child.
//! 5. The clip is popped.
//! 6. The specular hairline (white at `hairline_alpha`, 1 logical px, inset
//!    by half a pixel), when `hairline_alpha` is above zero.
//!
//! Material precedence is builder, then theme token, then the unthemed
//! fallback: [`GlassSurfaceView::material`] wins over the theme's tier, and
//! without either the surface paints an opaque neutral fill.
//!
//! The material's `blur_radius_intent` is a Gaussian standard deviation in
//! logical px, passed to the engine unconverted: a CSS `backdrop-filter:
//! blur(Npx)` is a sigma of `N`. The engine clamps it to its maximum sigma.
//!
//! # Backdrop limits
//!
//! The blur is not free and the engine bounds it; read these before placing
//! many surfaces.
//!
//! - **Per-frame cap.** The engine allows `MAX_BACKDROPS_PER_FRAME` backdrop
//!   blurs per frame by default; an app changes it (clamped to
//!   `BACKDROP_CAP_CEILING`) with `set_max_backdrops_per_frame`. The
//!   (cap + 1)th backdrop **refuses the whole frame**: the engine's refusal
//!   contract never degrades a frame silently, so a screen with too many
//!   glass surfaces shows nothing new rather than a partly blurred picture.
//! - **Dropped inside some layers.** Inside an opacity layer, a retained body
//!   or a suppressed layer the blur is dropped and only the washes paint (the
//!   `backdrop-blur-bound` limitation). Retained boundaries paint a body that
//!   contains a blur inline every frame, so such a body loses its page reuse.
//! - **Sigma clamp.** The engine clamps the blur to its maximum sigma.
//!
//! Keep glass surfaces outside fading or animated-opacity ancestors.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use frust_theme::{
    Brightness, ColorScheme, GlassMaterial, GlassTier, ShadowSpec, SurfaceRole, Theme,
};
use kurbo::{BezPath, Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

/// Flattening tolerance for the hairline's rounded-rect path.
const HAIRLINE_TOLERANCE: f64 = 0.1;
/// Hairline stroke width in logical px.
const HAIRLINE_WIDTH: f64 = 1.0;
/// The opaque fill when there is no theme to resolve a surface role from.
const FALLBACK_FILL: Color = Color::from_rgb8(0xEF, 0xEF, 0xEF);

/// A declarative glass surface. See the [module docs](self).
pub struct GlassSurfaceView<State: 'static> {
    child: AnyView<State>,
    tier: GlassTier,
    corner_radius: f64,
    material: Option<GlassMaterial>,
}

/// Wrap `child` in a glass surface painting the theme's [`GlassTier::Bar`]
/// tier; change it with [`GlassSurfaceView::tier`].
pub fn glass_surface<State: 'static>(child: impl View<State>) -> GlassSurfaceView<State> {
    GlassSurfaceView {
        child: any(child),
        tier: GlassTier::default(),
        corner_radius: 0.0,
        material: None,
    }
}

impl<State: 'static> GlassSurfaceView<State> {
    /// Which tier of the theme's glass scale to paint. [`GlassTier::Bar`] by
    /// default.
    pub fn tier(mut self, tier: GlassTier) -> Self {
        self.tier = tier;
        self
    }

    /// Corner radius of the clip, fills, shadow and hairline, in logical px.
    /// Square corners by default.
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Paint this material instead of the theme's tier. Wins over
    /// [`GlassSurfaceView::tier`] for the material itself; the tier still
    /// picks the surface role of the opaque fill.
    pub fn material(mut self, material: GlassMaterial) -> Self {
        self.material = Some(material);
        self
    }
}

/// The retained widget for a [`GlassSurfaceView`].
pub struct GlassSurfaceWidget {
    child: ChildPod,
    tier: GlassTier,
    corner_radius: f64,
    material: Option<GlassMaterial>,
}

impl<State: 'static> View<State> for GlassSurfaceView<State> {
    type Element = GlassSurfaceWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlassSurfaceWidget {
        GlassSurfaceWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            tier: self.tier,
            corner_radius: self.corner_radius,
            material: self.material.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlassSurfaceWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.tier != self.tier
            || prev.corner_radius != self.corner_radius
            || prev.material != self.material
        {
            element.tier = self.tier;
            element.corner_radius = self.corner_radius;
            element.material = self.material.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut GlassSurfaceWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

/// `color` with its alpha multiplied by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * alpha])
}

/// The scheme color for a surface-container `role`.
fn surface_color(scheme: &ColorScheme, role: SurfaceRole) -> Color {
    match role {
        SurfaceRole::Surface => scheme.surface,
        SurfaceRole::SurfaceContainerLowest => scheme.surface_container_lowest,
        SurfaceRole::SurfaceContainerLow => scheme.surface_container_low,
        SurfaceRole::SurfaceContainer => scheme.surface_container,
        SurfaceRole::SurfaceContainerHigh => scheme.surface_container_high,
        SurfaceRole::SurfaceContainerHighest => scheme.surface_container_highest,
    }
}

/// The clip the shadow draws under, in the surface's local space: an outer
/// rect covering the shadow's full extent with the surface's rounded rect as
/// a hole. The hole's winding is reversed so the nonzero clip excludes it.
/// `None` when the outer rect is not finite (a finite but huge blur or
/// offset), since a non-finite clip path would refuse the whole frame.
fn shadow_clip_path(size: Size, radius: f64, y_offset: f64, blur_std_dev: f64) -> Option<BezPath> {
    let blur = if blur_std_dev.is_finite() {
        blur_std_dev.max(0.0)
    } else {
        0.0
    };
    let offset = if y_offset.is_finite() {
        y_offset.abs()
    } else {
        0.0
    };
    let margin = 3.0 * blur + offset + 1.0;
    let outer = Rect::new(-margin, -margin, size.width + margin, size.height + margin);
    if ![outer.x0, outer.y0, outer.x1, outer.y1]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    let mut path = outer.to_path(HAIRLINE_TOLERANCE);
    let hole =
        RoundedRect::new(0.0, 0.0, size.width, size.height, radius).to_path(HAIRLINE_TOLERANCE);
    let hole = hole.reverse_subpaths();
    path.extend(hole);
    Some(path)
}

impl GlassSurfaceWidget {
    /// The material to paint, or `None` when there is neither an explicit one
    /// nor a theme.
    fn resolved_material<'a>(&'a self, theme: Option<&'a Theme>) -> Option<&'a GlassMaterial> {
        self.material
            .as_ref()
            .or_else(|| theme.map(|t| t.glass.tier(self.tier)))
    }

    /// The opaque fill: the tier's elevation surface role from the theme's
    /// active scheme, or a neutral color without a theme.
    fn opaque_fill(&self, theme: Option<&Theme>) -> Color {
        let Some(theme) = theme else {
            return FALLBACK_FILL;
        };
        let level = match self.tier {
            GlassTier::Control => &theme.elevation.level1,
            GlassTier::Bar => &theme.elevation.level2,
            GlassTier::Chrome => &theme.elevation.level3,
        };
        surface_color(theme.scheme(), level.surface_role)
    }
}

impl Widget for GlassSurfaceWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin();
        let size = ctx.size();
        let radius = if self.corner_radius.is_finite() {
            self.corner_radius.max(0.0)
        } else {
            0.0
        };
        let material = self.resolved_material(theme);
        let hairline_alpha = material.map_or(0.0, |m| m.hairline_alpha);

        if let Some(material) = material {
            let ShadowSpec {
                y_offset,
                blur_std_dev,
                color_alpha,
            } = material.shadow;
            if color_alpha > 0.0 {
                let base = theme.map_or(Color::BLACK, |t| t.scheme().shadow);
                // Clip the shadow to everything outside the surface so it is
                // not painted under the lens or the washes.
                let clip = shadow_clip_path(size, radius, y_offset, blur_std_dev);
                if let Some(path) = &clip {
                    scene.push_clip_path(origin, path);
                }
                scene.draw_shadow(
                    Point::new(origin.x, origin.y + y_offset),
                    size,
                    radius,
                    blur_std_dev,
                    with_alpha(base, color_alpha),
                );
                if clip.is_some() {
                    scene.pop_clip();
                }
            }
        }

        scene.push_clip_rounded(origin, size, radius);
        match material.filter(|m| !m.is_opaque()) {
            Some(material) => {
                scene.backdrop_blur(origin, size, material.blur_radius_intent);
                let brightness = theme.map_or(Brightness::default(), |t| t.brightness);
                let fills = match brightness {
                    Brightness::Light => &material.fills_light,
                    Brightness::Dark => &material.fills_dark,
                };
                for fill in fills {
                    scene.fill_rounded_rect(origin, size, radius, fill.color);
                }
            }
            None => scene.fill_rounded_rect(origin, size, radius, self.opaque_fill(theme)),
        }
        self.child.paint_child(ctx, scene);
        scene.pop_clip();

        if hairline_alpha > 0.0 {
            let inset = HAIRLINE_WIDTH / 2.0;
            let rect = RoundedRect::new(
                inset,
                inset,
                (size.width - inset).max(inset),
                (size.height - inset).max(inset),
                (radius - inset).max(0.0),
            );
            let color = Color::new([1.0, 1.0, 1.0, hairline_alpha]);
            scene.stroke_path(
                origin,
                &rect.to_path(HAIRLINE_TOLERANCE),
                HAIRLINE_WIDTH,
                &Brush::Solid(color),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use frust_theme::{DesignLanguage, GlassFill};

    #[derive(Debug, PartialEq)]
    enum Op {
        ShadowClip(Point, BezPath),
        Shadow(Point, Size, f64, f64, Color),
        Clip(Point, Size, f64),
        Blur(Point, Size, f64),
        Fill(f64, Color),
        Child(Size),
        PopClip,
        Hairline(Point, f64, Brush),
    }

    #[derive(Default)]
    struct Rec {
        ops: Vec<Op>,
    }

    impl PaintScene for Rec {
        fn fill_rect(&mut self, _o: Point, s: Size, _c: Color) {
            self.ops.push(Op::Child(s));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, r: f64, c: Color) {
            self.ops.push(Op::Fill(r, c));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, r: f64, sd: f64, c: Color) {
            self.ops.push(Op::Shadow(o, s, r, sd, c));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, r: f64) {
            self.ops.push(Op::Clip(o, s, r));
        }
        fn push_clip_path(&mut self, o: Point, p: &BezPath) {
            self.ops.push(Op::ShadowClip(o, p.clone()));
        }
        fn pop_clip(&mut self) {
            self.ops.push(Op::PopClip);
        }
        fn backdrop_blur(&mut self, o: Point, s: Size, r: f64) {
            self.ops.push(Op::Blur(o, s, r));
        }
        fn stroke_path(&mut self, o: Point, _p: &BezPath, w: f64, b: &Brush) {
            self.ops.push(Op::Hairline(o, w, b.clone()));
        }
    }

    fn material(blur: f64) -> GlassMaterial {
        GlassMaterial {
            blur_radius_intent: blur,
            fills_light: vec![
                GlassFill::new(1.0, 1.0, 1.0, 0.3),
                GlassFill::new(0.9, 0.9, 0.9, 0.1),
            ],
            fills_dark: vec![GlassFill::new(0.0, 0.0, 0.0, 0.4)],
            hairline_alpha: 0.3,
            shadow: ShadowSpec {
                y_offset: 2.0,
                blur_std_dev: 4.0,
                color_alpha: 0.12,
            },
        }
    }

    /// A cupertino-shaped theme: every tier translucent with its own blur.
    fn glass_theme() -> Theme {
        Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Cupertino)
            .map_glass(|mut g| {
                g.control = material(15.0);
                g.bar = material(45.0);
                g.chrome = material(75.0);
                g
            })
            .build()
    }

    fn paint_with(view: &GlassSurfaceView<()>, theme: Option<&Theme>) -> Vec<Op> {
        let mut counter = 0u64;
        let mut w = view.build(&mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let mut ctx = PaintCtx::new(Point::new(10.0, 20.0), size);
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        let mut rec = Rec::default();
        w.paint(&mut ctx, &mut rec);
        rec.ops
    }

    fn surface() -> GlassSurfaceView<()> {
        glass_surface(leaf(40.0, 30.0))
    }

    #[test]
    fn translucent_tier_records_clipped_shadow_clip_blur_fills_child_hairline_in_order() {
        let theme = glass_theme();
        let ops = paint_with(&surface().corner_radius(8.0), Some(&theme));
        let o = Point::new(10.0, 20.0);
        let s = Size::new(40.0, 30.0);
        let shadow_color = with_alpha(theme.scheme().shadow, 0.12);
        assert_eq!(ops.len(), 10, "{ops:?}");
        assert_eq!(
            ops[0],
            Op::ShadowClip(o, shadow_clip_path(s, 8.0, 2.0, 4.0).unwrap())
        );
        assert_eq!(
            ops[1],
            Op::Shadow(Point::new(10.0, 22.0), s, 8.0, 4.0, shadow_color)
        );
        assert_eq!(ops[2], Op::PopClip);
        assert_eq!(ops[3], Op::Clip(o, s, 8.0));
        assert_eq!(ops[4], Op::Blur(o, s, 45.0));
        assert_eq!(ops[5], Op::Fill(8.0, Color::new([1.0, 1.0, 1.0, 0.3])));
        assert_eq!(ops[6], Op::Fill(8.0, Color::new([0.9, 0.9, 0.9, 0.1])));
        assert_eq!(ops[7], Op::Child(s));
        assert_eq!(ops[8], Op::PopClip);
        assert_eq!(
            ops[9],
            Op::Hairline(o, 1.0, Brush::Solid(Color::new([1.0, 1.0, 1.0, 0.3])))
        );
    }

    #[test]
    fn blur_radius_is_the_tier_intent() {
        let theme = glass_theme();
        for (tier, sigma) in [
            (GlassTier::Control, 15.0),
            (GlassTier::Bar, 45.0),
            (GlassTier::Chrome, 75.0),
        ] {
            let ops = paint_with(&surface().tier(tier), Some(&theme));
            let blur = ops.iter().find_map(|op| match op {
                Op::Blur(_, _, r) => Some(*r),
                _ => None,
            });
            assert_eq!(blur, Some(sigma));
        }
    }

    #[test]
    fn dark_brightness_uses_fills_dark() {
        let theme = glass_theme().with_brightness(Brightness::Dark);
        let ops = paint_with(&surface(), Some(&theme));
        let fills: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Fill(_, c) => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(fills, vec![Color::new([0.0, 0.0, 0.0, 0.4])]);
    }

    #[test]
    fn explicit_material_wins_over_the_theme_tier() {
        let theme = glass_theme();
        let ops = paint_with(&surface().material(material(7.0)), Some(&theme));
        assert!(ops.contains(&Op::Blur(
            Point::new(10.0, 20.0),
            Size::new(40.0, 30.0),
            7.0
        )));
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Blur(_, _, r) if *r == 45.0))
        );
    }

    #[test]
    fn opaque_theme_records_no_backdrop_and_fills_the_surface_role() {
        let theme = Theme::neutral();
        let ops = paint_with(&surface().tier(GlassTier::Chrome), Some(&theme));
        assert!(!ops.iter().any(|op| matches!(op, Op::Blur(..))));
        let role = theme.elevation.level3.surface_role;
        let expected = surface_color(theme.scheme(), role);
        assert!(ops.contains(&Op::Fill(0.0, expected)));
        assert!(!ops.iter().any(|op| matches!(op, Op::Hairline(..))));
    }

    #[test]
    fn shadow_clip_excludes_the_surface_interior() {
        let s = Size::new(40.0, 30.0);
        let path = shadow_clip_path(s, 8.0, 2.0, 4.0).unwrap();
        // Nonzero winding: the outer rect wraps +1, the reversed hole cancels.
        let inside = Point::new(20.0, 15.0);
        let ring = Point::new(-5.0, 15.0);
        let outer = Point::new(-100.0, 15.0);
        assert_eq!(kurbo::Shape::winding(&path, inside), 0);
        assert_ne!(kurbo::Shape::winding(&path, ring), 0);
        assert_eq!(kurbo::Shape::winding(&path, outer), 0);
        // The outer rect covers the whole shadow extent (3 sigma plus offset).
        assert!(kurbo::Shape::bounding_box(&path).contains(Point::new(-14.0, 36.0)));
    }

    #[test]
    fn a_shadow_margin_too_large_to_represent_draws_the_shadow_unclipped() {
        let theme = glass_theme();
        // 3 x 1e308 and MAX + MAX/2 both overflow the margin.
        for (y_offset, blur_std_dev) in [(2.0, 1e308), (f64::MAX, f64::MAX / 2.0)] {
            assert!(shadow_clip_path(Size::new(40.0, 30.0), 8.0, y_offset, blur_std_dev).is_none());
            let mut m = material(12.0);
            m.shadow = ShadowSpec {
                y_offset,
                blur_std_dev,
                color_alpha: 0.12,
            };
            let ops = paint_with(&surface().material(m), Some(&theme));
            assert!(
                !ops.iter().any(|op| matches!(op, Op::ShadowClip(..))),
                "{ops:?}"
            );
            assert!(matches!(ops[0], Op::Shadow(..)), "{ops:?}");
            // The shadow's push/pop stays balanced: only the surface clip pops.
            assert_eq!(ops.iter().filter(|op| **op == Op::PopClip).count(), 1);
        }
    }

    #[test]
    fn non_finite_or_non_positive_intent_paints_opaque_with_no_backdrop() {
        let theme = glass_theme();
        for blur in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0] {
            let ops = paint_with(&surface().material(material(blur)), Some(&theme));
            assert!(!ops.iter().any(|op| matches!(op, Op::Blur(..))), "{blur}");
            let expected = surface_color(theme.scheme(), theme.elevation.level2.surface_role);
            assert!(ops.contains(&Op::Fill(0.0, expected)), "{blur}");
        }
    }

    #[test]
    fn no_theme_falls_back_to_an_opaque_fill() {
        let ops = paint_with(&surface(), None);
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Blur(..) | Op::Shadow(..)))
        );
        assert_eq!(
            ops,
            vec![
                Op::Clip(Point::new(10.0, 20.0), Size::new(40.0, 30.0), 0.0),
                Op::Fill(0.0, FALLBACK_FILL),
                Op::Child(Size::new(40.0, 30.0)),
                Op::PopClip,
            ]
        );
    }
}
