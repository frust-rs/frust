//! The `Icon` widget: paints a vector icon from `kurbo::BezPath` path data,
//! scaled from its design box to a requested logical size.
//!
//! [`icon`] takes any [`IconData`] — either a generated [`IconSource`] from the
//! vendored Material Symbols set ([`crate::icons`]) or a user-built
//! [`BezPath`](kurbo::BezPath) via [`IconData::from_path`] — and paints it as a
//! single filled path via [`PaintScene::fill_path`]. Material Symbols outlines
//! are pre-flattened fills, so no stroking is involved.
//!
//! # Stroked, multi-element icons
//!
//! A line-art icon set (a stroke on a fixed grid, several elements per icon,
//! circles and rects alongside paths) is expressed as a [`StrokedIconSource`]:
//! a `const`-constructible list of [`IconElement`]s, each an [`IconShape`]
//! painted as a stroke or a fill ([`IconPaint`]), plus the design box and the
//! stroke width in design units. Strokes use round caps and joins via
//! [`PaintScene::stroke_path`], scale with the icon (a 1.6-wide stroke on a 24
//! grid draws 3.2 px at size 48), and every element is inked with the same
//! resolved icon color as a filled icon. Runtime-built shapes go through
//! [`IconData::from_elements`]. [`IconSource`] is unchanged.
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

use frust_core::accesskit::Role;
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View,
    Widget,
};
use frust_theme::Theme;
use kurbo::{Affine, BezPath, Circle, Rect as KRect, RoundedRect, Shape, Size};
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

/// One geometric primitive of a stroked icon, in design-box coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconShape {
    /// An SVG path `d` string.
    Path(&'static str),
    /// A circle centred on `(cx, cy)` with radius `r`.
    Circle {
        /// Centre x.
        cx: f64,
        /// Centre y.
        cy: f64,
        /// Radius.
        r: f64,
    },
    /// An axis-aligned rectangle at `(x, y)` of size `w`×`h`, with corner
    /// radius `rx` (`0.0` for square corners).
    Rect {
        /// Left edge.
        x: f64,
        /// Top edge.
        y: f64,
        /// Width.
        w: f64,
        /// Height.
        h: f64,
        /// Corner radius.
        rx: f64,
    },
}

/// How an [`IconElement`] is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IconPaint {
    /// Stroked with the icon's stroke width, round caps and joins.
    #[default]
    Stroke,
    /// Filled (nonzero).
    Fill,
}

/// A shape plus how it is painted; one entry of a [`StrokedIconSource`].
#[derive(Clone, Copy, Debug)]
pub struct IconElement {
    /// The geometry.
    pub shape: IconShape,
    /// Stroke or fill.
    pub paint: IconPaint,
}

impl IconElement {
    /// A stroked element.
    pub const fn stroke(shape: IconShape) -> Self {
        Self {
            shape,
            paint: IconPaint::Stroke,
        }
    }

    /// A filled element.
    pub const fn fill(shape: IconShape) -> Self {
        Self {
            shape,
            paint: IconPaint::Fill,
        }
    }
}

/// A stroked, multi-element icon: `const`-constructible, so a line-art set can
/// live in `pub const` tables. Convert it into an [`IconData`] (via `From`, or
/// by passing it to [`icon`]) to paint it.
///
/// Stroked icons are not resolvable to a single filled path: see
/// [`IconData::resolve`].
#[derive(Clone, Copy, Debug)]
pub struct StrokedIconSource {
    /// The elements, painted in order.
    pub elements: &'static [IconElement],
    /// The side length of the square design box the elements are authored against.
    pub design: f64,
    /// The stroke width, in design units (scaled with the icon at paint time).
    pub stroke_width: f64,
}

/// Flattening tolerance, in design units, for circle and rect elements.
const SHAPE_TOLERANCE: f64 = 0.1;

impl IconShape {
    /// Convert to a design-space path. A malformed `Path` string is a wiring
    /// bug and panics.
    fn to_bez_path(self) -> BezPath {
        match self {
            IconShape::Path(d) => BezPath::from_svg(d).unwrap_or_else(|e| {
                panic!(
                    "stroked icon SVG path failed to parse (wiring bug — a malformed \
                     StrokedIconSource element): {e}"
                )
            }),
            IconShape::Circle { cx, cy, r } => Circle::new((cx, cy), r).to_path(SHAPE_TOLERANCE),
            IconShape::Rect { x, y, w, h, rx } => {
                if rx > 0.0 {
                    RoundedRect::new(x, y, x + w, y + h, rx).to_path(SHAPE_TOLERANCE)
                } else {
                    KRect::new(x, y, x + w, y + h).to_path(SHAPE_TOLERANCE)
                }
            }
        }
    }
}

/// Parsed multi-element geometry, shared behind an `Arc`.
#[derive(Debug)]
struct ElementsGeom {
    elements: Vec<(BezPath, IconPaint)>,
    design: f64,
    stroke_width: f64,
}

/// A user-supplied path plus its design box, shared behind an `Arc` so an
/// [`IconData`] clone is cheap.
#[derive(Debug)]
struct PathGeom {
    path: BezPath,
    design: f64,
}

/// The shapes an [`IconData`] can hold.
#[derive(Clone, Debug)]
enum IconRepr {
    /// A generated static source: parse its `d` lazily at build/rebuild.
    Svg { d: &'static str, design: f64 },
    /// A user-built path + design box, already in `kurbo` form.
    Path(Arc<PathGeom>),
    /// A generated static stroked source: parsed lazily at build/rebuild.
    Stroked(StrokedIconSource),
    /// Runtime-built multi-element geometry.
    Elements(Arc<ElementsGeom>),
}

/// The resolved geometry an [`IconWidget`] paints.
enum Geometry {
    /// One filled path.
    Fill(BezPath),
    /// Several elements, each stroked or filled, at `stroke_width` design units.
    Elements {
        elements: Vec<(BezPath, IconPaint)>,
        stroke_width: f64,
    },
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
    /// stored in an `Arc`, so cloning the resulting [`IconData`] (as a build
    /// closure does every frame) is a refcount bump, never a copy of the geometry.
    pub fn from_path(path: BezPath, design_size: f64) -> Self {
        Self {
            repr: IconRepr::Path(Arc::new(PathGeom {
                path,
                design: design_size,
            })),
        }
    }

    /// Build a stroked, multi-element icon from already-built paths, each
    /// stroked or filled, a design box and a stroke width in design units.
    /// Shared behind an `Arc` like [`IconData::from_path`].
    pub fn from_elements(
        elements: Vec<(BezPath, IconPaint)>,
        design_size: f64,
        stroke_width: f64,
    ) -> Self {
        Self {
            repr: IconRepr::Elements(Arc::new(ElementsGeom {
                elements,
                design: design_size,
                stroke_width,
            })),
        }
    }

    /// Whether this is a stroked, multi-element icon (which [`IconData::resolve`]
    /// cannot represent).
    pub fn is_stroked(&self) -> bool {
        matches!(self.repr, IconRepr::Stroked(_) | IconRepr::Elements(_))
    }

    /// Resolve into the geometry the widget paints, plus the design box.
    fn geometry(&self) -> (Geometry, f64) {
        match &self.repr {
            IconRepr::Stroked(src) => (
                Geometry::Elements {
                    elements: src
                        .elements
                        .iter()
                        .map(|e| (e.shape.to_bez_path(), e.paint))
                        .collect(),
                    stroke_width: src.stroke_width,
                },
                src.design,
            ),
            IconRepr::Elements(geom) => (
                Geometry::Elements {
                    elements: geom.elements.clone(),
                    stroke_width: geom.stroke_width,
                },
                geom.design,
            ),
            _ => {
                let (path, design) = self.resolve();
                (Geometry::Fill(path), design)
            }
        }
    }

    /// Resolve this handle into a concrete `(design-space path, design box)`
    /// pair.
    ///
    /// For a generated [`IconSource`] this parses its `d` string via
    /// [`BezPath::from_svg`](kurbo::BezPath::from_svg); a parse failure is a
    /// wiring bug (a malformed generated entry) and panics. For a user path it
    /// clones the shared `BezPath` (cheap for the small paths icons are).
    ///
    /// # Panics
    ///
    /// On a stroked or multi-element icon ([`StrokedIconSource`],
    /// [`IconData::from_elements`]): those are not resolvable to one filled
    /// path, so asking is a wiring bug. Fill-only consumers (a plugin's icon
    /// slot) do not support stroked icons; check [`IconData::is_stroked`]
    /// first if the source is not known.
    ///
    /// Public because out-of-tree design systems (the design-system plugin
    /// tier) paint icons through it — `IconData` without `resolve` has no
    /// reachable geometry outside this crate.
    pub fn resolve(&self) -> (BezPath, f64) {
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
            IconRepr::Stroked(_) | IconRepr::Elements(_) => panic!(
                "stroked icons are not resolvable to one filled path (wiring bug — \
                 a fill-only consumer was given a stroked icon)"
            ),
        }
    }

    /// Whether `self` and `other` name the same icon geometry, cheaply — an
    /// `Arc` pointer check for user paths, a `d`/design comparison for generated
    /// sources. Lets [`IconView::rebuild`] skip re-parsing an unchanged icon.
    ///
    /// Public for the same reason as [`IconData::resolve`]: out-of-tree design
    /// systems need the cheap-identity check to skip re-parsing on rebuild.
    pub fn same(&self, other: &IconData) -> bool {
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
            (IconRepr::Stroked(a), IconRepr::Stroked(b)) => {
                a.design == b.design
                    && a.stroke_width == b.stroke_width
                    && (std::ptr::eq(a.elements, b.elements)
                        || a.elements
                            .iter()
                            .zip(b.elements)
                            .all(|(x, y)| x.paint == y.paint && x.shape == y.shape)
                            && a.elements.len() == b.elements.len())
            }
            (IconRepr::Elements(a), IconRepr::Elements(b)) => Arc::ptr_eq(a, b),
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

impl From<StrokedIconSource> for IconData {
    fn from(source: StrokedIconSource) -> Self {
        IconData {
            repr: IconRepr::Stroked(source),
        }
    }
}

impl From<&StrokedIconSource> for IconData {
    fn from(source: &StrokedIconSource) -> Self {
        IconData::from(*source)
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
    stroke_width: Option<f64>,
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
        stroke_width: None,
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

    /// Override a stroked icon's stroke width, in design units (scaled with
    /// the icon). Ignored by filled icons.
    pub fn stroke_width(mut self, width: f64) -> Self {
        self.stroke_width = Some(width);
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
        let (geometry, design) = self.data.geometry();
        IconWidget {
            data: self.data.clone(),
            geometry,
            design,
            size: self.size,
            // Natural-size default until `layout` runs — a widget painted
            // before its first `layout` pass (or a design-system consumer
            // that skips it, e.g. this module's own paint-only tests) sees
            // the unconstrained `size×size` box, matching pre-fix behavior.
            laid_out_size: Size::new(self.size, self.size),
            color: self.color,
            stroke_width: self.stroke_width,
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
            let (geometry, design) = self.data.geometry();
            element.data = self.data.clone();
            element.geometry = geometry;
            element.design = design;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            // Re-seed the natural-size default (see `build`'s comment); the
            // LAYOUT flag below re-measures it against real constraints
            // before the next paint in the normal pipeline, but a caller
            // that paints without laying out first still sees the new size.
            element.laid_out_size = Size::new(self.size, self.size);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if prev.stroke_width != self.stroke_width {
            element.stroke_width = self.stroke_width;
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
    /// The parsed geometry, in its design-box coordinate space (`0..design`).
    geometry: Geometry,
    /// The side length of the geometry's square design box.
    design: f64,
    size: f64,
    /// The box `layout` last resolved (`bc.constrain(size×size)`) — what
    /// `paint` actually fills. Matches `size×size` exactly under a loose
    /// constraint (the natural, overwhelmingly common case); diverges under a
    /// tight constraint the requested `size` doesn't satisfy.
    laid_out_size: Size,
    color: Option<Color>,
    /// Explicit stroke-width override, in design units.
    stroke_width: Option<f64>,
    label: Option<String>,
}

impl Widget for IconWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A fixed size×size box, clamped into the incoming constraints (a tight
        // constraint — e.g. inside a SizedBox — wins outright).
        let laid_out = bc.constrain(Size::new(self.size, self.size));
        self.laid_out_size = laid_out;
        laid_out
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let color = self
            .color
            .unwrap_or_else(|| resolve_default_color(Theme::from_paint_ctx(ctx)));
        // Scale the design box to fill the LAID-OUT box (min-side scale, so a
        // non-square laid-out box never overdraws either axis), then center
        // the scaled glyph inside it. Under a loose constraint (the common
        // case) `laid_out_size == size×size`, so `scale == size/design` and
        // the centering offset is exactly zero — byte-identical to the
        // pre-fix, size-only scale. `design` is always > 0 for a real icon;
        // guard against a degenerate design box just in case.
        let scale = if self.design > 0.0 {
            self.laid_out_size.width.min(self.laid_out_size.height) / self.design
        } else {
            1.0
        };
        let glyph_side = self.design * scale;
        let dx = (self.laid_out_size.width - glyph_side) / 2.0;
        let dy = (self.laid_out_size.height - glyph_side) / 2.0;
        let transform = Affine::translate((dx, dy)) * Affine::scale(scale);
        let brush = Brush::Solid(color);
        match &self.geometry {
            Geometry::Fill(path) => {
                scene.fill_path(ctx.origin(), &(transform * path.clone()), &brush);
            }
            Geometry::Elements {
                elements,
                stroke_width,
            } => {
                let width = self.stroke_width.unwrap_or(*stroke_width) * scale;
                for (path, paint) in elements {
                    let scaled = transform * path.clone();
                    match paint {
                        IconPaint::Stroke => {
                            scene.stroke_path(ctx.origin(), &scaled, width, &brush)
                        }
                        IconPaint::Fill => scene.fill_path(ctx.origin(), &scaled, &brush),
                    }
                }
            }
        }
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
    use frust_core::BuildCtx;
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
        strokes: Vec<(Point, Rect, f64, Color)>,
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
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((origin, path.bounding_box(), width, color));
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
    fn paint_natural_size_after_layout_is_unchanged() {
        // Guards the common case: laying out under a loose constraint (the
        // overwhelmingly common shape) resolves to size×size, and paint must
        // still fill exactly that box with zero centering offset — the same
        // output as `paint_scales_the_design_box_to_the_requested_size`
        // above, but with `layout` actually run first.
        let view = icon(square_data()).size(48.0);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let laid_out = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(laid_out, Size::new(48.0, 48.0));
        let rec = paint_rec(&mut w, Point::new(3.0, 5.0), None);
        let (origin, bbox, _) = rec.fills[0];
        assert_eq!(origin, Point::new(3.0, 5.0));
        assert!((bbox.width() - 48.0).abs() < 1e-6, "path scaled to size");
        assert!((bbox.height() - 48.0).abs() < 1e-6);
        assert!((bbox.x0 - 0.0).abs() < 1e-6);
        assert!((bbox.y0 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn paint_fills_a_tight_larger_box_centered() {
        // Tight-LARGER (G12, fab_menu's collapsed trigger): a 34×34 tight
        // constraint around a 24-design icon. Pre-fix, `paint` scaled to the
        // VIEW's configured size (24) and drew at the box origin, leaving
        // the glyph off-center by (34-24)/2 = 5dp toward the top-left. Fixed
        // paint must fill the laid-out 34×34 box instead.
        let view = icon(square_data()); // default size 24.0, design 24.0
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let laid_out = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(34.0, 34.0)));
        assert_eq!(laid_out, Size::new(34.0, 34.0));
        let rec = paint_rec(&mut w, Point::new(3.0, 5.0), None);
        let (origin, bbox, _) = rec.fills[0];
        assert_eq!(origin, Point::new(3.0, 5.0));
        assert!(
            (bbox.width() - 34.0).abs() < 1e-6,
            "glyph must fill the laid-out box (34), not the requested size (24); got {}",
            bbox.width()
        );
        assert!((bbox.height() - 34.0).abs() < 1e-6);
        // Centered: a square design box scaled to fill a square laid-out box
        // needs zero translation — the scaled path starts flush at 0..34.
        assert!((bbox.x0 - 0.0).abs() < 1e-6);
        assert!((bbox.y0 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn paint_fills_a_tight_smaller_box_without_overdraw() {
        // Tight-SMALLER: a 16×16 tight constraint around a 24-design icon —
        // the latent overdraw bug (same root cause as G12, never
        // device-observed since no shipped consumer constrains an icon
        // smaller than its requested size, but broken all the same). Pre-fix
        // `paint` drew the full 24×24 glyph regardless, overdrawing past the
        // laid-out box's edges.
        let view = icon(square_data()); // default size 24.0, design 24.0
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let laid_out = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(16.0, 16.0)));
        assert_eq!(laid_out, Size::new(16.0, 16.0));
        let rec = paint_rec(&mut w, Point::new(3.0, 5.0), None);
        let (origin, bbox, _) = rec.fills[0];
        assert_eq!(origin, Point::new(3.0, 5.0));
        assert!(
            (bbox.width() - 16.0).abs() < 1e-6,
            "glyph must shrink to the laid-out box (16), not overdraw at the \
             requested size (24); got {}",
            bbox.width()
        );
        assert!((bbox.height() - 16.0).abs() < 1e-6);
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
        let theme = Theme::neutral();
        let rec = paint_rec(&mut w, Point::ZERO, Some(&theme));
        assert_eq!(rec.fills[0].2, explicit);
    }

    #[test]
    fn themed_default_resolves_on_surface() {
        let view = icon(square_data());
        let mut w = build(&view);
        let theme = Theme::neutral();
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
    fn the_icon_button_arc_additions_exist_and_are_in_all() {
        // The two directional arrows the caret precedent doesn't cover, a
        // third pointing forward, and a copy affordance — all four are
        // present in the generated set and enumerable via `ALL`, not just
        // reachable by name.
        let additions = super::super::icons::ALL;
        for (name, d) in [
            ("ARROW_UPWARD", super::super::icons::ARROW_UPWARD.d),
            ("ARROW_DOWNWARD", super::super::icons::ARROW_DOWNWARD.d),
            ("ARROW_FORWARD", super::super::icons::ARROW_FORWARD.d),
            ("CONTENT_COPY", super::super::icons::CONTENT_COPY.d),
        ] {
            assert!(
                additions.iter().any(|source| source.d == d),
                "icons::{name} is missing from icons::ALL"
            );
        }
    }

    #[test]
    fn generated_home_icon_is_usable_end_to_end() {
        // `icon(icons::HOME).size(28.0)`
        // builds and paints in a bare-core (no-theme) test.
        let view = icon(super::super::icons::HOME).size(28.0);
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.fills.len(), 1);
        assert_eq!(rec.fills[0].2, DEFAULT_COLOR);
    }

    // -- stroked icons ------------------------------------------------------

    const LINE_ICON: StrokedIconSource = StrokedIconSource {
        elements: &[
            IconElement::stroke(IconShape::Path("M4 4 L20 4")),
            IconElement::stroke(IconShape::Circle {
                cx: 12.0,
                cy: 12.0,
                r: 3.0,
            }),
        ],
        design: 24.0,
        stroke_width: 1.6,
    };

    #[test]
    fn stroked_icon_strokes_each_element_with_scaled_width() {
        let view = icon(LINE_ICON).size(48.0);
        let mut w = build(&view);
        let theme = Theme::neutral();
        let rec = paint_rec(&mut w, Point::ZERO, Some(&theme));
        assert!(rec.fills.is_empty());
        assert_eq!(rec.strokes.len(), 2);
        for (_, _, width, color) in &rec.strokes {
            assert!((width - 3.2).abs() < 1e-9);
            assert_eq!(*color, theme.scheme().on_surface);
        }
    }

    #[test]
    fn stroke_width_override_replaces_the_source_width() {
        let view = icon(LINE_ICON).size(48.0).stroke_width(1.0);
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert!((rec.strokes[0].2 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn circle_and_rect_elements_become_paths() {
        let data = IconData::from(
            const {
                StrokedIconSource {
                    elements: &[
                        IconElement::stroke(IconShape::Circle {
                            cx: 12.0,
                            cy: 12.0,
                            r: 3.0,
                        }),
                        IconElement::stroke(IconShape::Rect {
                            x: 2.0,
                            y: 4.0,
                            w: 10.0,
                            h: 6.0,
                            rx: 0.0,
                        }),
                        IconElement::stroke(IconShape::Rect {
                            x: 2.0,
                            y: 4.0,
                            w: 10.0,
                            h: 6.0,
                            rx: 2.0,
                        }),
                    ],
                    design: 24.0,
                    stroke_width: 1.0,
                }
            },
        );
        let view = icon(data).size(48.0);
        let mut w = build(&view);
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.strokes.len(), 3);
        let near = |a: f64, b: f64| (a - b).abs() < 0.2;
        let c = rec.strokes[0].1;
        assert!(near(c.x0, 18.0) && near(c.x1, 30.0) && near(c.y0, 18.0) && near(c.y1, 30.0));
        for i in [1, 2] {
            let r = rec.strokes[i].1;
            assert!(near(r.x0, 4.0) && near(r.x1, 24.0) && near(r.y0, 8.0) && near(r.y1, 20.0));
        }
    }

    #[test]
    fn fill_elements_in_a_stroked_icon_use_fill_path() {
        let data = IconData::from(
            const {
                StrokedIconSource {
                    elements: &[
                        IconElement::stroke(IconShape::Path("M0 0 L24 24")),
                        IconElement::fill(IconShape::Circle {
                            cx: 12.0,
                            cy: 12.0,
                            r: 2.0,
                        }),
                    ],
                    design: 24.0,
                    stroke_width: 1.6,
                }
            },
        );
        let mut w = build(&icon(data));
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.fills.len(), 1);
    }

    #[test]
    fn explicit_color_wins_for_stroked_icons() {
        let explicit = Color::from_rgb8(0xAB, 0xCD, 0xEF);
        let mut w = build(&icon(LINE_ICON).color(explicit));
        let theme = Theme::neutral();
        let rec = paint_rec(&mut w, Point::ZERO, Some(&theme));
        assert!(rec.strokes.iter().all(|s| s.3 == explicit));
    }

    #[test]
    fn no_theme_uses_default_color() {
        let mut w = build(&icon(LINE_ICON));
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert!(rec.strokes.iter().all(|s| s.3 == DEFAULT_COLOR));
    }

    #[test]
    fn stroked_source_is_const_usable() {
        const T: StrokedIconSource = StrokedIconSource {
            elements: &[IconElement::stroke(IconShape::Circle {
                cx: 12.0,
                cy: 12.0,
                r: 3.0,
            })],
            design: 24.0,
            stroke_width: 1.6,
        };
        let mut w = build(&icon(T));
        assert_eq!(paint_rec(&mut w, Point::ZERO, None).strokes.len(), 1);
    }

    #[test]
    fn from_elements_paints_runtime_shapes() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 10.0));
        let data = IconData::from_elements(vec![(p, IconPaint::Stroke)], 10.0, 1.0);
        let mut w = build(&icon(data).size(20.0));
        let rec = paint_rec(&mut w, Point::ZERO, None);
        assert_eq!(rec.strokes.len(), 1);
        assert!((rec.strokes[0].2 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn same_distinguishes_stroked_icons() {
        let a = IconData::from(LINE_ICON);
        assert!(a.same(&IconData::from(LINE_ICON)));
        let other = StrokedIconSource {
            stroke_width: 2.0,
            ..LINE_ICON
        };
        assert!(!a.same(&IconData::from(other)));
        assert!(!a.same(&square_data()));
        assert!(a.is_stroked() && !square_data().is_stroked());
    }

    #[test]
    #[should_panic(expected = "not resolvable to one filled path")]
    fn resolve_on_a_stroked_icon_panics() {
        let _ = IconData::from(LINE_ICON).resolve();
    }

    #[test]
    #[should_panic(expected = "wiring bug")]
    fn malformed_stroked_path_panics_as_wiring_bug() {
        let data = IconData::from(
            const {
                StrokedIconSource {
                    elements: &[IconElement::stroke(IconShape::Path("not a path"))],
                    design: 24.0,
                    stroke_width: 1.6,
                }
            },
        );
        let _ = build(&icon(data));
    }

    // -- semantics ----------------------------------------------------------

    /// Slack allowed on either side of the `0..MATERIAL_DESIGN_BOX` design box
    /// when checking a generated icon's parsed geometry.
    ///
    /// `scripts/gen_icons.py`'s `normalize_path_d` renormalizes each glyph's
    /// coordinates from its source SVG's `viewBox` into the `0..24` box via an
    /// affine transform, but a handful of upstream Material Symbols exports
    /// author raw path data that itself extends a little past their own
    /// declared `viewBox` (optical overshoot at a glyph's rounded tips) — an
    /// independent Python bbox walker confirmed that, for a handful of icons,
    /// a few percent past the box is genuine upstream geometry, not a
    /// transform bug. A direct measurement of the 41 icons
    /// currently vendored in `icons/mod.rs` (this test, run standalone) finds
    /// zero icons actually bleeding past `0..24` today — every glyph's bbox
    /// currently lands strictly inside — so this constant is pure headroom:
    /// wide enough (a "few percent" of 24px is well under 1px) to tolerate a
    /// future regeneration reintroducing that documented upstream overshoot
    /// without needing a bump, while still tight enough to catch a real
    /// transform regression (e.g. a wrong `viewBox` scale/offset putting a
    /// whole glyph noticeably outside the box).
    const BBOX_TOLERANCE: f64 = 2.0;

    #[test]
    fn every_generated_icon_bbox_lands_in_design_box() {
        let mut max_bleed: f64 = f64::NEG_INFINITY;
        let mut worst: Option<&str> = None;
        for source in super::super::icons::ALL {
            let path = BezPath::from_svg(source.d).unwrap_or_else(|e| {
                panic!("icon SVG path failed to parse: {e} (d={:?})", source.d)
            });
            let bbox = path.bounding_box();
            let design = source.design;
            assert!(
                bbox.x0 >= -BBOX_TOLERANCE
                    && bbox.y0 >= -BBOX_TOLERANCE
                    && bbox.x1 <= design + BBOX_TOLERANCE
                    && bbox.y1 <= design + BBOX_TOLERANCE,
                "icon `{}` bbox ({:.4},{:.4})-({:.4},{:.4}) lands outside the \
                 {design}x{design} design box beyond the {BBOX_TOLERANCE}px \
                 tolerance",
                source.d,
                bbox.x0,
                bbox.y0,
                bbox.x1,
                bbox.y1
            );
            // Non-degenerate check: a zero (or near-zero) scale bug would
            // collapse every glyph's bbox to a point/sliver.
            assert!(
                bbox.width() + bbox.height() > design / 4.0,
                "icon `{}` bbox ({:.4},{:.4})-({:.4},{:.4}) is degenerately \
                 small for a {design}x{design} design box — suspect a \
                 zero-scale bug",
                source.d,
                bbox.x0,
                bbox.y0,
                bbox.x1,
                bbox.y1
            );
            let bleed = [-bbox.x0, -bbox.y0, bbox.x1 - design, bbox.y1 - design, 0.0]
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max);
            if bleed > max_bleed {
                max_bleed = bleed;
                worst = Some(source.d);
            }
        }
        // Informational only (not asserted): surfaces the worst observed
        // bleed when run with `-- --nocapture`, for eyeballing against
        // BBOX_TOLERANCE's headroom.
        if let Some(d) = worst {
            println!("max bbox bleed observed: {max_bleed:.4}px (icon d={d:?})");
        }
    }

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
