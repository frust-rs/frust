//! The `Icon` widget: paints a vector icon from `kurbo::BezPath` path data,
//! scaled from its design box to a requested logical size (spec §6.4, Huddle
//! showcase task 02).
//!
//! [`icon`] takes any [`IconData`] — either a generated [`IconSource`] from the
//! vendored Material Symbols set ([`crate::icons`]) or a user-built
//! [`BezPath`](kurbo::BezPath) via [`IconData::from_path`] — and paints it as a
//! single filled path via [`PaintScene::fill_path`]. Material Symbols outlines
//! are pre-flattened fills, so no stroking is involved.
//!
//! # Color resolution
//!
//! An icon's color follows the framework's **explicit builder value > theme >
//! fallback** precedence (see `docs/CODE_STANDARDS.md`'s Theming conventions):
//! an explicit [`IconView::color`] always wins; otherwise the default resolves
//! `on_surface` from the threaded [`Theme`], falling back to [`DEFAULT_COLOR`]
//! when no theme is present (bare-core tests, pre-theme apps).
//!
//! # Path parsing
//!
//! A generated [`IconSource`] carries its geometry as an SVG path `d` string,
//! parsed via [`BezPath::from_svg`](kurbo::BezPath::from_svg) at widget
//! build/rebuild and cached on the widget. A parse failure is a **wiring bug**
//! (a malformed generated entry), not a runtime-data condition, so it panics
//! with a message saying so — per `docs/CODE_STANDARDS.md`.

use std::sync::Arc;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use forgekit_theme::Theme;
use kurbo::{Affine, BezPath, Size};
use peniko::{Brush, Color};

/// Default icon side length, in logical px (Material Symbols' 24×24 design box
/// at 1:1).
const DEFAULT_SIZE: f64 = 24.0;

/// Unthemed default icon color (matches the M3 light `on_surface` role, so a
/// pre-theme app renders the same as the themed light default). A theme
/// resolves this from `scheme().on_surface`.
const DEFAULT_COLOR: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// A generated icon: SVG path `d` data plus the side length of its square
/// design box.
///
/// This is the format `scripts/gen_icons.py` emits into [`crate::icons`] — one
/// `pub const <NAME>: IconSource = IconSource { d: "...", design: 24.0 };`
/// entry per glyph. Convert it into an [`IconData`] to paint it (via the `From`
/// impls, or simply by passing it to [`icon`], which takes `impl Into<IconData>`).
#[derive(Clone, Copy, Debug)]
pub struct IconSource {
    /// The SVG `d` path attribute, in the design box's coordinate space
    /// (y-down, `0..design` on each axis).
    pub d: &'static str,
    /// The side length of the square design box `d` is authored against.
    pub design: f64,
}

/// A user-supplied path plus its design box, shared behind an `Arc` so an
/// [`IconData`] clone is cheap.
#[derive(Debug)]
struct PathGeom {
    path: BezPath,
    design: f64,
}

/// The two shapes an [`IconData`] can hold.
#[derive(Clone, Debug)]
enum IconRepr {
    /// A generated static source: parse its `d` lazily at build/rebuild.
    Svg { d: &'static str, design: f64 },
    /// A user-built path + design box, already in `kurbo` form.
    Path(Arc<PathGeom>),
}

/// A cheap-clone handle around an icon's path data and its design box.
///
/// Construct one from a generated [`IconSource`] (via [`From`], or implicitly
/// through [`icon`]) or from any [`BezPath`](kurbo::BezPath) via
/// [`IconData::from_path`] — the composability seam that lets an app supply its
/// own icons without going through the generated set.
#[derive(Clone, Debug)]
pub struct IconData {
    repr: IconRepr,
}

impl IconData {
    /// Build icon data from an arbitrary `kurbo::BezPath` and the side length of
    /// the square design box it was authored against.
    ///
    /// This is the composability requirement: an app can paint any vector shape
    /// as an icon, not just the vendored Material Symbols set. The path is
    /// stored in an `Arc`, so cloning the resulting [`IconData`] (as `app_logic`
    /// does every frame) is a refcount bump, never a copy of the geometry.
    pub fn from_path(path: BezPath, design_size: f64) -> Self {
        Self {
            repr: IconRepr::Path(Arc::new(PathGeom {
                path,
                design: design_size,
            })),
        }
    }

    /// Resolve this handle into a concrete `(design-space path, design box)`
    /// pair.
    ///
    /// For a generated [`IconSource`] this parses its `d` string via
    /// [`BezPath::from_svg`](kurbo::BezPath::from_svg); a parse failure is a
    /// wiring bug (a malformed generated entry) and panics. For a user path it
    /// clones the shared `BezPath` (cheap for the small paths icons are).
    fn resolve(&self) -> (BezPath, f64) {
        match &self.repr {
            IconRepr::Svg { d, design } => {
                let path = BezPath::from_svg(d).unwrap_or_else(|e| {
                    panic!(
                        "icon SVG path failed to parse (wiring bug — a malformed \
                         generated IconSource): {e}"
                    )
                });
                (path, *design)
            }
            IconRepr::Path(geom) => (geom.path.clone(), geom.design),
        }
    }

    /// Whether `self` and `other` name the same icon geometry, cheaply — an
    /// `Arc` pointer check for user paths, a `d`/design comparison for generated
    /// sources. Lets [`IconView::rebuild`] skip re-parsing an unchanged icon.
    fn same(&self, other: &IconData) -> bool {
        match (&self.repr, &other.repr) {
            (
                IconRepr::Svg {
                    d: a,
                    design: design_a,
                },
                IconRepr::Svg {
                    d: b,
                    design: design_b,
                },
            ) => a == b && design_a == design_b,
            (IconRepr::Path(a), IconRepr::Path(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl From<IconSource> for IconData {
    fn from(source: IconSource) -> Self {
        IconData {
            repr: IconRepr::Svg {
                d: source.d,
                design: source.design,
            },
        }
    }
}

impl From<&IconSource> for IconData {
    fn from(source: &IconSource) -> Self {
        IconData::from(*source)
    }
}

/// The resolved default icon color: `on_surface` from the threaded theme, or the
/// [`DEFAULT_COLOR`] fallback when no theme is present.
fn resolve_default_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => DEFAULT_COLOR,
    }
}

/// A declarative icon. See the [module docs](self).
///
/// Not generic over app state — an icon carries no callbacks, so like
/// [`ImageView`](crate::ImageView) it implements `View<State>` for every
/// `State`.
pub struct IconView {
    data: IconData,
    size: f64,
    color: Option<Color>,
    label: Option<String>,
}

/// Create an icon view over any [`IconData`] source (a generated
/// [`IconSource`], or a user path via [`IconData::from_path`]), at the default
/// 24.0 logical size with the theme's `on_surface` color.
pub fn icon(data: impl Into<IconData>) -> IconView {
    IconView {
        data: data.into(),
        size: DEFAULT_SIZE,
        color: None,
        label: None,
    }
}

/// PascalCase alias for [`icon`].
#[allow(non_snake_case)]
pub fn Icon(data: impl Into<IconData>) -> IconView {
    icon(data)
}

impl IconView {
    /// Set the icon's side length, in logical px (default 24.0). The design box
    /// is scaled uniformly to this size at paint time.
    pub fn size(mut self, size: f64) -> Self {
        self.size = size;
        self
    }

    /// Set an explicit icon color, overriding the theme's default `on_surface`
    /// (explicit builder value wins — see the [module docs](self)).
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Attach an accessible label, contributing a [`Role::Image`] semantics node
    /// (an unlabelled icon is decorative and reports nothing).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl<State: 'static> View<State> for IconView {
    type Element = IconWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> IconWidget {
        let (base_path, design) = self.data.resolve();
        IconWidget {
            data: self.data.clone(),
            base_path,
            design,
            size: self.size,
            color: self.color,
            label: self.label.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut IconWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if !prev.data.same(&self.data) {
            // Only a genuine icon change re-parses; an every-frame rebuild that
            // re-supplies the same source keeps the cached path.
            let (base_path, design) = self.data.resolve();
            element.data = self.data.clone();
            element.base_path = base_path;
            element.design = design;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            // Semantics are recomputed each frame from the widget, so the label
            // just needs to be adopted — no layout/paint dirtiness.
            element.label = self.label.clone();
        }
        flags
    }
}

/// The retained widget for an [`IconView`].
pub struct IconWidget {
    /// Retained for `rebuild`'s cheap same-icon check (avoids re-parsing).
    data: IconData,
    /// The parsed path, in its design-box coordinate space (`0..design`).
    base_path: BezPath,
    /// The side length of `base_path`'s square design box.
    design: f64,
    size: f64,
    color: Option<Color>,
    label: Option<String>,
}

impl Widget for IconWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A fixed size×size box, clamped into the incoming constraints (a tight
        // constraint — e.g. inside a SizedBox — wins outright).
        bc.constrain(Size::new(self.size, self.size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let color = self
            .color
            .unwrap_or_else(|| resolve_default_color(Theme::from_paint_ctx(ctx)));
        // Scale the design box to the laid-out size. `design` is always > 0 for
        // a real icon; guard against a degenerate design box just in case.
        let scale = if self.design > 0.0 {
            self.size / self.design
        } else {
            1.0
        };
        let scaled = Affine::scale(scale) * self.base_path.clone();
        scene.fill_path(ctx.origin(), &scaled, &Brush::Solid(color));
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // An unlabelled icon is decorative and contributes nothing; a labelled
        // one is a single Image node carrying its accessible name.
        if let Some(label) = &self.label {
            ctx.push_node(Role::Image, |node| {
                node.set_label(label.as_str());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::BuildCtx;
    use kurbo::{Point, Rect, Shape};

    /// The design-box side length every vendored Material Symbols icon is
    /// authored against (a 24×24 viewBox).
    const MATERIAL_DESIGN_BOX: f64 = 24.0;

    /// A 24×24 design-box square, as a user-supplied path with a known bounding
    /// box (so a scale check is deterministic regardless of a real glyph's
    /// extent).
    fn square_data() -> IconData {
        let path = Rect::new(0.0, 0.0, MATERIAL_DESIGN_BOX, MATERIAL_DESIGN_BOX).to_path(0.1);
        IconData::from_path(path, MATERIAL_DESIGN_BOX)
    }

    fn build(view: &IconView) -> IconWidget {
        let mut counter = 0u64;
        <IconView as View<()>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// A recording [`PaintScene`] capturing every `fill_path`'s origin, the
    /// filled path's bounding box, and its solid color.
    #[derive(Default)]
    struct RecordingScene {
        fills: Vec<(Point, Rect, Color)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.fills.push((origin, path.bounding_box(), color));
        }
    }

    fn paint_rec(w: &mut IconWidget, origin: Point, theme: Option<&Theme>) -> RecordingScene {
        let mut rec = RecordingScene::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(origin, Size::new(w.size, w.size)).with_theme(t),
            None => PaintCtx::new(origin, Size::new(w.size, w.size)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // -- layout ------------------------------------------------------------

    #[test]
    fn loose_constraints_use_the_requested_size() {
        let view = icon(super::super::icons::HOME).size(28.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(28.0, 28.0));
    }

    #[test]
    fn tight_constraints_win_over_the_requested_size() {
        let view = icon(super::super::icons::HOME).size(28.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(40.0, 40.0)));
        assert_eq!(size, Size::new(40.0, 40.0));
    }

    #[test]
    fn default_size_is_24() {
        let view = icon(super::super::icons::HOME);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(24.0, 24.0));
    }

    // -- paint / scaling ----------------------------------------------------

    #[test]
    fn paint_scales_the_design_box_to_the_requested_size() {
        // A 24×24 design box painted at size 48 should fill a 0..48 box, offset
        // by the paint origin.
        let view = icon(square_data()).size(48.0);
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::new(3.0, 5.0), None);
        assert_eq!(rec.fills.len(), 1);
        let (origin, bbox, _) = rec.fills[0];
        assert_eq!(origin, Point::new(3.0, 5.0));
        assert!((bbox.width() - 48.0).abs() < 1e-6, "path scaled to size");
        assert!((bbox.height() - 48.0).abs() < 1e-6);
        // The path itself is in local space (origin applied by the scene).
        assert!((bbox.x0 - 0.0).abs() < 1e-6);
        assert!((bbox.y0 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn user_supplied_path_renders() {
        // The composability requirement: a hand-built BezPath paints as an icon.
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();
        let view = icon(IconData::from_path(path, 10.0)).size(20.0);
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.fills.len(), 1, "a user path emits one filled path");
        let (_, bbox, _) = rec.fills[0];
        // 10-wide design box scaled 2x -> 20 wide.
        assert!((bbox.width() - 20.0).abs() < 1e-6);
    }

    // -- color resolution ---------------------------------------------------

    #[test]
    fn explicit_color_wins_over_theme() {
        let explicit = Color::from_rgb8(0xAB, 0xCD, 0xEF);
        let view = icon(square_data()).color(explicit);
        let mut w = build(&view);
        let theme = Theme::m3_baseline();
        let rec = paint_rec(&mut w, Point::ZERO, Some(&theme));
        assert_eq!(rec.fills[0].2, explicit);
    }

    #[test]
    fn themed_default_resolves_on_surface() {
        let view = icon(square_data());
        let mut w = build(&view);
        let theme = Theme::m3_baseline();
        let rec = paint_rec(&mut w, Point::ZERO, Some(&theme));
        assert_eq!(rec.fills[0].2, theme.scheme().on_surface);
    }

    #[test]
    fn unthemed_default_uses_fallback_constant() {
        let view = icon(square_data());
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.fills[0].2, DEFAULT_COLOR);
    }

    // -- generated set ------------------------------------------------------

    #[test]
    fn every_generated_icon_source_parses() {
        for source in super::super::icons::ALL {
            let data: IconData = (*source).into();
            let (path, design) = data.resolve();
            assert_eq!(design, MATERIAL_DESIGN_BOX);
            assert!(
                !path.elements().is_empty(),
                "generated icon `{}` parsed to an empty path",
                source.d
            );
        }
    }

    #[test]
    fn generated_home_icon_is_usable_end_to_end() {
        // Mirrors the task's acceptance criterion: `icon(icons::HOME).size(28.0)`
        // builds and paints in a bare-core (no-theme) test.
        let view = icon(super::super::icons::HOME).size(28.0);
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.fills.len(), 1);
        assert_eq!(rec.fills[0].2, DEFAULT_COLOR);
    }

    // -- semantics ----------------------------------------------------------

    #[test]
    fn rebuild_adopts_a_new_size_and_flags_layout() {
        let mut counter = 0u64;
        let prev = icon(square_data()).size(24.0);
        let mut w = <IconView as View<()>>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = icon(square_data()).size(32.0);
        let flags =
            <IconView as View<()>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.size, 32.0);
        assert!(flags.needs_layout());
        assert!(flags.needs_paint());
    }
}
