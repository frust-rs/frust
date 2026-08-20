//! Shapes: the reference's `ShapesPlayground`.
//!
//! Stateless, like the reference (`ShapesPlayground extends StatelessWidget`)
//! — nothing here is a knob, so this page carries no [`frust::Component`].
//!
//! `frust_material::shapes` has no `M3EShapeContainer`/`M3EShapeClipper`
//! widget yet, only the geometry ([`ShapeKind`]/`RoundedPolygon`) itself —
//! see that module's own docs. [`shape_tile`]/[`fit_polygon_path`] below are
//! this page's own stand-in: a small leaf widget filling a `ShapeKind`'s
//! outline into a fixed box, non-uniformly scaled to fit — the same fit the
//! reference's `M3EShapeClipper.getClip` computes. It **fills** rather than
//! **clips**: `PaintScene` has no arbitrary-path clip primitive (only a
//! rect/rounded-rect one), so the "Clipped child" preview overlays its icon
//! centered on top of the filled shape instead of true-clipping it to the
//! outline.

use frust::authoring::text::TextAlign;
use frust::authoring::{
    Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx,
    PaintScene, Shape as PathShape, Size, Vec2, View, Widget,
};
use frust::{
    Align, Alignment, AnyView, Column, CrossAxisAlignment, Row, SizedBox, Stack, Theme, any, icon,
    text,
};
use frust_material::icons;
use frust_material::shapes::ShapeKind;

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, play_preview_card, play_snippet, playground_body,
};

/// Catalog tile side length, in logical px — the reference's own tile
/// `SizedBox(width: 72, height: 72)`.
const CATALOG_TILE_SIZE: f64 = 72.0;
/// Gap between adjacent catalog tiles, in logical px — the reference's `Wrap`
/// `spacing`/`runSpacing`.
const CATALOG_TILE_GAP: f64 = 12.0;
/// Tiles per catalog row. This port has no reflowing `Wrap` layout (see the
/// module docs), so the 35-shape grid is chunked into fixed rows instead.
const CATALOG_COLUMNS: usize = 5;
/// "Clipped child" preview side length, in logical px — the reference's own
/// `width: 120, height: 120`.
const CLIPPED_TILE_SIZE: f64 = 120.0;

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    let theme = ambient_theme();
    playground_body(
        vec![catalog_preview(&theme), clipped_child_preview(&theme)],
        vec![catalog_snippet(), clipped_child_snippet()],
        vec![],
    )
}

/// The 35-shape catalog, chunked [`CATALOG_COLUMNS`] wide — the reference's
/// `Wrap` of `M3EShapeContainer` tiles.
fn catalog_preview(theme: &Theme) -> AnyView<AppState> {
    let mut rows: Vec<AnyView<AppState>> = Vec::new();
    for (row_index, chunk) in ShapeKind::ALL.chunks(CATALOG_COLUMNS).enumerate() {
        if row_index > 0 {
            rows.push(any(SizedBox::<AppState>(None, Some(16.0))));
        }
        let mut cells: Vec<AnyView<AppState>> = Vec::new();
        for (col_index, kind) in chunk.iter().enumerate() {
            if col_index > 0 {
                cells.push(any(SizedBox::<AppState>(Some(CATALOG_TILE_GAP), None)));
            }
            cells.push(catalog_tile(theme, *kind));
        }
        rows.push(any(Row(cells)));
    }
    any(play_preview_card("Catalog", Column(rows)))
}

/// One catalog tile: a filled shape over its label — the reference's own
/// `Column` of `M3EShapeContainer` + `Text(kind.name)`.
fn catalog_tile(theme: &Theme, kind: ShapeKind) -> AnyView<AppState> {
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.label_small.clone();
    label_style.color = scheme.on_surface_variant;
    any(Column(vec![
        any(shape_tile(
            kind,
            CATALOG_TILE_SIZE,
            scheme.primary_container,
        )),
        any(SizedBox::<AppState>(None, Some(6.0))),
        any(SizedBox::<AppState>(Some(CATALOG_TILE_SIZE), None).child(
            text(shape_label(kind))
                .style(label_style)
                .align(TextAlign::Center),
        )),
    ])
    .cross_axis(CrossAxisAlignment::Center))
}

/// A shape's catalog label — the reference's own `kind.name` (its enum's
/// camelCase variant name); this port's [`ShapeKind`] has no `Display`, so
/// this reads its `Debug` (PascalCase) name instead.
fn shape_label(kind: ShapeKind) -> String {
    format!("{kind:?}")
}

/// A shape filled with a centered icon on top — approximating (not
/// reproducing) the reference's
/// `M3EShapeContainer.cookie4Sided(child: Icon(...))`; see the module docs
/// for why this overlays rather than clips.
fn clipped_child_preview(theme: &Theme) -> AnyView<AppState> {
    let scheme = theme.scheme();
    let tile =
        SizedBox::<AppState>(Some(CLIPPED_TILE_SIZE), Some(CLIPPED_TILE_SIZE)).child(Stack(vec![
            any(shape_tile(
                ShapeKind::Cookie4Sided,
                CLIPPED_TILE_SIZE,
                scheme.tertiary_container,
            )),
            any(Align(
                Alignment::CENTER,
                icon(icons::FAVORITE).color(scheme.on_tertiary_container),
            )),
        ]));
    any(play_preview_card("Clipped child", tile))
}

fn catalog_snippet() -> PlaySnippet {
    play_snippet(
        "Catalog tile",
        "let path = ShapeKind::Circle.polygon().to_path(); // scaled to the tile's own box\n\
         scene.fill_path(origin, &path, &Brush::Solid(theme.scheme().primary_container));",
    )
}

fn clipped_child_snippet() -> PlaySnippet {
    play_snippet(
        "Shape + icon overlay",
        "Stack(vec![\n\
         \u{20}   any(shape_tile(ShapeKind::Cookie4Sided, 120.0, theme.scheme().tertiary_container)),\n\
         \u{20}   any(Align(Alignment::CENTER, icon(icons::FAVORITE)\n\
         \u{20}       .color(theme.scheme().on_tertiary_container))),\n\
         ]);\n\
         // `PaintScene` has no arbitrary-path clip yet, so this overlays rather\n\
         // than true-clips the icon to the shape outline.",
    )
}

/// Fill `kind`'s outline into a `size`×`size` box, tinted `color` — this
/// page's own stand-in for the reference's `M3EShapeContainer` (see the
/// module docs).
fn shape_tile(kind: ShapeKind, size: f64, color: Color) -> ShapeFillView {
    ShapeFillView { kind, size, color }
}

/// The declarative half of [`shape_tile`].
struct ShapeFillView {
    kind: ShapeKind,
    size: f64,
    color: Color,
}

impl<State: 'static> View<State> for ShapeFillView {
    type Element = ShapeFillWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ShapeFillWidget {
        ShapeFillWidget {
            kind: self.kind,
            size: self.size,
            color: self.color,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ShapeFillWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.kind != self.kind || prev.size != self.size {
            element.kind = self.kind;
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained half of [`shape_tile`].
struct ShapeFillWidget {
    kind: ShapeKind,
    size: f64,
    color: Color,
}

impl Widget for ShapeFillWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.size, self.size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let path = fit_polygon_path(self.kind, ctx.size());
        scene.fill_path(ctx.origin(), &path, &Brush::Solid(self.color));
    }
}

/// Non-uniformly scale + translate `kind`'s `RoundedPolygon` path to fill
/// `size`, top-left aligned — the same fit the reference's
/// `M3EShapeClipper.getClip` computes (see the module docs).
fn fit_polygon_path(kind: ShapeKind, size: Size) -> BezPath {
    let raw = kind.polygon().to_path();
    let bounds = raw.bounding_box();
    if size.width <= 0.0 || size.height <= 0.0 || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return BezPath::new();
    }
    let scale = Affine::new([
        size.width / bounds.width(),
        0.0,
        0.0,
        size.height / bounds.height(),
        0.0,
        0.0,
    ]);
    let scaled = scale * raw;
    let scaled_bounds = scaled.bounding_box();
    Affine::translate(Vec2::new(-scaled_bounds.x0, -scaled_bounds.y0)) * scaled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_builds() {
        let entry = crate::catalog::find_by_id("shapes").expect("catalog entry exists");
        let _view = page(entry);
    }

    #[test]
    fn every_catalog_shape_fits_into_a_non_empty_path() {
        for kind in ShapeKind::ALL {
            let path = fit_polygon_path(kind, Size::new(CATALOG_TILE_SIZE, CATALOG_TILE_SIZE));
            assert!(
                !path.elements().is_empty(),
                "{kind:?} produced an empty fitted path"
            );
        }
    }

    #[test]
    fn a_degenerate_box_returns_an_empty_path_rather_than_panicking() {
        let path = fit_polygon_path(ShapeKind::Circle, Size::new(0.0, 0.0));
        assert!(path.elements().is_empty());
    }
}
