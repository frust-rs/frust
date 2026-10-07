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
    Align, Alignment, AnyView, Column, CrossAxisAlignment, Row, SizedBox, Theme, any, column, icon,
    stack, text,
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
/// module docs), so the 35-shape grid is chunked into fixed rows instead —
/// each `Row` of [`CATALOG_COLUMNS`] tiles is a plain `Row` of all-inflexible
/// fixed-width children (the G10 overflow class documented at
/// `crate::pages::playground::find::progress`'s module docs), so the column
/// count must be small enough that a full row's natural width fits a
/// phone-ish card's ~296px inner width (`360 - 4 * MaterialSpacing::LG`),
/// computed from [`CATALOG_TILE_SIZE`]/[`CATALOG_TILE_GAP`] rather than
/// guessed: 4 columns is `4 * 72 + 3 * 12 = 324 > 296` (overflows — this was
/// G17's bug, at the previous count of 5), 3 columns is
/// `3 * 72 + 2 * 12 = 240 <= 296` (fits).
const CATALOG_COLUMNS: usize = 3;
/// "Clipped child" preview side length, in logical px — the reference's own
/// `width: 120, height: 120`.
const CLIPPED_TILE_SIZE: f64 = 120.0;

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    let theme = ambient_theme();
    any(playground_body(
        vec![catalog_preview(&theme), clipped_child_preview(&theme)],
        vec![catalog_snippet(), clipped_child_snippet()],
        vec![],
    ))
}

/// The 35-shape catalog, chunked [`CATALOG_COLUMNS`] wide — the reference's
/// `Wrap` of `M3EShapeContainer` tiles. Split into [`catalog_rows`] (this
/// file's G17 regression test exercises it directly at the card's own inner
/// width) and this wrapper, the same split
/// `crate::pages::playground::find::progress`'s `all_styles_row`/G10 fix and
/// `crate::pages::playground::view::cards`'s `all_variants_column`/G16 fix
/// both use.
fn catalog_preview(theme: &Theme) -> AnyView<AppState> {
    play_preview_card("Catalog", catalog_rows(theme))
}

/// The tile grid [`catalog_preview`] wraps in a [`play_preview_card`]. See
/// [`CATALOG_COLUMNS`]' own doc comment for the row-width arithmetic (G17).
fn catalog_rows(theme: &Theme) -> impl View<AppState> {
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
            cells.push(any(catalog_tile(theme, *kind)));
        }
        rows.push(any(Row(cells)));
    }
    Column(rows)
}

/// One catalog tile: a filled shape over its label — the reference's own
/// `Column` of `M3EShapeContainer` + `Text(kind.name)`.
fn catalog_tile(theme: &Theme, kind: ShapeKind) -> impl View<AppState> {
    let scheme = theme.scheme();
    let mut label_style = theme.type_scale.label_small.clone();
    label_style.color = scheme.on_surface_variant;
    column()
        .child(shape_tile(
            kind,
            CATALOG_TILE_SIZE,
            scheme.primary_container,
        ))
        .child(SizedBox::<AppState>(None, Some(6.0)))
        .child(
            SizedBox::<AppState>(Some(CATALOG_TILE_SIZE), None).child(
                text(shape_label(kind))
                    .style(label_style)
                    .align(TextAlign::Center),
            ),
        )
        .cross_axis(CrossAxisAlignment::Center)
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
    let tile = SizedBox::<AppState>(Some(CLIPPED_TILE_SIZE), Some(CLIPPED_TILE_SIZE)).child(
        stack()
            .child(shape_tile(
                ShapeKind::Cookie4Sided,
                CLIPPED_TILE_SIZE,
                scheme.tertiary_container,
            ))
            .child(Align(
                Alignment::CENTER,
                icon(icons::FAVORITE).color(scheme.on_tertiary_container),
            )),
    );
    play_preview_card("Clipped child", tile)
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

    /// G17 regression: [`catalog_rows`] must never paint past the "Catalog"
    /// card's real inner width at a phone-ish device width — the same
    /// paint-extent idiom `crate::pages::playground::find::progress`'s G10
    /// test uses.
    ///
    /// Reverting the fix (back to [`CATALOG_COLUMNS`] = 5) fails this test:
    /// a full row of 5 tiles (`5 * 72 + 4 * 12 = 408`) paints straight
    /// through a phone-ish card's ~296px inner width.
    #[test]
    fn catalog_rows_never_paints_past_the_cards_inner_width() {
        use std::any::Any;

        use frust::authoring::Point;
        use frust::authoring::text::TextContext;

        const DEVICE_WIDTH: f64 = 360.0;
        let card_inner_width = DEVICE_WIDTH - 4.0 * frust_material::MaterialSpacing::LG;

        #[derive(Default)]
        struct MaxXScene {
            max_x: f64,
        }
        impl PaintScene for MaxXScene {
            fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
            fn draw_text(&mut self, _origin: Point, _text: &str) {}
            fn fill_path(&mut self, origin: Point, path: &BezPath, _brush: &Brush) {
                self.max_x = self.max_x.max(origin.x + path.bounding_box().x1);
            }
        }

        let theme = frust_material::baseline();
        let view = catalog_rows(&theme);
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));
        let bc = BoxConstraints::new(Size::ZERO, Size::new(card_inner_width, 4000.0));
        let mut text_ctx = TextContext::new();
        let mut layout_ctx = LayoutCtx::with_resources(
            Some(&mut text_ctx as &mut dyn Any),
            Some(&theme as &dyn Any),
        );
        let size = widget.layout(&mut layout_ctx, &bc);
        assert!(
            size.width <= card_inner_width + 1e-6,
            "the catalog's own reported size {size:?} must not exceed the \
             inner width {card_inner_width}"
        );

        let mut ctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        let mut scene = MaxXScene::default();
        widget.paint(&mut ctx, &mut scene);
        assert!(
            scene.max_x <= card_inner_width + 1e-6,
            "G17 regressed: painted x {} exceeds the card's inner width {} \
             (reported size {size:?})",
            scene.max_x,
            card_inner_width
        );
    }
}
