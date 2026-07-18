//! Showcase · Theme screen — REAL (wave-2 task 04, ported from
//! `examples/gallery`).
//!
//! A Material 3 color-role swatch grid, the 15-token type scale rendered as
//! styled text, and real gaussian-blurred elevation shadows over `surface` —
//! `examples/gallery`'s theme page (spec §17), reading the ambient
//! [`Theme`] the shell already threads (no brightness toggle here: task 06's
//! settings screen owns the app-forced light/dark override).
//!
//! # Why this file depends on `forgekit-core`/`kurbo`/`peniko` directly
//!
//! No facade widget paints an arbitrary-color filled rect or a real blurred
//! drop shadow, so [`ColorBoxView`] below is the same small `View`/`Widget`
//! pair `examples/gallery` built directly against `forgekit-core` (spec §6)
//! — the facade's documented "low-level escape hatch" pattern
//! (`forgekit::App::new`, see `crates/forgekit/src/lib.rs`) applied one
//! layer down, kept file-local per the task. Everything else here —
//! layout containers, reading the theme via `use_context::<Theme>()` — goes
//! through the `forgekit` facade exactly like every other screen in this
//! crate.

use forgekit::{
    AnyView, ColorScheme, Column, EdgeInsets, ElevationLevel, Padding, Row, ShadowSpec, SizedBox,
    SurfaceRole, Theme, any, scroll_view, text, use_context,
};
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Point, Size};
use peniko::Color;

use crate::ShellState;

// --- ColorBoxView/Widget: a flat swatch, or (with a shadow) an elevation card ---

/// A themed rectangle: a flat color swatch (`shadow: None`), or — with a
/// [`ShadowSpec`] attached via [`ColorBoxView::shadow`] — a real
/// gaussian-blurred elevation card. See the module docs for why this is a
/// small hand-rolled `View`/`Widget` pair rather than a facade widget.
struct ColorBoxView {
    size: Size,
    fill: Color,
    radius: f64,
    shadow: Option<(ShadowSpec, Color)>,
}

fn color_box(size: Size, fill: Color, radius: f64) -> ColorBoxView {
    ColorBoxView {
        size,
        fill,
        radius,
        shadow: None,
    }
}

impl ColorBoxView {
    /// Attach a real blurred elevation shadow (drawn under the fill, offset
    /// by `spec.y_offset`), turning the swatch into an elevation card.
    fn shadow(mut self, spec: ShadowSpec, color: Color) -> Self {
        self.shadow = Some((spec, color));
        self
    }
}

/// The retained widget for a [`ColorBoxView`]. Childless leaf: fixed size,
/// filled rect, optional shadow.
struct ColorBoxWidget {
    size: Size,
    fill: Color,
    radius: f64,
    shadow: Option<(ShadowSpec, Color)>,
}

impl<State: 'static> View<State> for ColorBoxView {
    type Element = ColorBoxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ColorBoxWidget {
        ColorBoxWidget {
            size: self.size,
            fill: self.fill,
            radius: self.radius,
            shadow: self.shadow,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ColorBoxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.size = self.size;
        element.fill = self.fill;
        element.radius = self.radius;
        element.shadow = self.shadow;
        ChangeFlags::PAINT
    }
}

impl Widget for ColorBoxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        if let Some((spec, color)) = self.shadow {
            let shadow_origin = Point::new(origin.x, origin.y + spec.y_offset);
            scene.draw_shadow(
                shadow_origin,
                size,
                self.radius,
                spec.blur_std_dev,
                color.with_alpha(spec.color_alpha),
            );
        }
        scene.fill_rounded_rect(origin, size, self.radius, self.fill);
    }
}

// --- The theme exhibit ---

/// The theme exhibit: M3 color-role swatch grid, 15-token type scale,
/// elevation shadows — reading the ambient [`Theme`] (no brightness toggle
/// here; task 06's settings screen owns the app-forced override).
pub fn theme_screen() -> AnyView<ShellState> {
    let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
    let scheme = *theme.scheme();
    any(scroll_view(Column(theme_page(&theme, scheme))))
}

/// Which `ColorScheme` field a [`SurfaceRole`] names (the M3 static
/// surface-container direction — see `forgekit-theme::elevation`'s module
/// docs).
fn surface_role_color(scheme: ColorScheme, role: SurfaceRole) -> Color {
    match role {
        SurfaceRole::Surface => scheme.surface,
        SurfaceRole::SurfaceContainerLowest => scheme.surface_container_lowest,
        SurfaceRole::SurfaceContainerLow => scheme.surface_container_low,
        SurfaceRole::SurfaceContainer => scheme.surface_container,
        SurfaceRole::SurfaceContainerHigh => scheme.surface_container_high,
        SurfaceRole::SurfaceContainerHighest => scheme.surface_container_highest,
    }
}

/// One labeled swatch cell: a flat [`ColorBoxView`] plus a caption below.
fn swatch_cell(scheme: ColorScheme, color: Color, label: &'static str) -> AnyView<ShellState> {
    any(Padding(
        EdgeInsets::all(6.0),
        Column(vec![
            any(color_box(Size::new(84.0, 48.0), color, 8.0)),
            any(text(label).size(12.0).color(scheme.on_surface)),
        ]),
    ))
}

/// One elevation card cell: a shadowed [`ColorBoxView`] at the level's mapped
/// surface-container role, plus a caption.
fn elevation_cell(
    theme: &Theme,
    scheme: ColorScheme,
    level: ElevationLevel,
    label: &'static str,
) -> AnyView<ShellState> {
    let fill = surface_role_color(scheme, level.surface_role);
    any(Padding(
        EdgeInsets::all(6.0),
        Column(vec![
            any(color_box(Size::new(96.0, 64.0), fill, theme.shape.medium)
                .shadow(level.shadow, scheme.shadow)),
            any(text(label).size(12.0).color(scheme.on_surface)),
        ]),
    ))
}

/// The theme page: color swatch grid, 15-token type scale, elevation cards.
fn theme_page(theme: &Theme, scheme: ColorScheme) -> Vec<AnyView<ShellState>> {
    let mut v: Vec<AnyView<ShellState>> = vec![
        any(text("Theme").size(28.0).color(scheme.on_surface)),
        any(
            text("Material 3 color roles, type scale, and elevation — the ambient theme.")
                .size(14.0)
                .color(scheme.on_surface),
        ),
        any(SizedBox(None, Some(12.0))),
        any(text("Color roles").size(20.0).color(scheme.on_surface)),
    ];
    let swatches = [
        (scheme.primary_container, "primary container"),
        (scheme.secondary_container, "secondary container"),
        (scheme.tertiary_container, "tertiary container"),
        (scheme.error_container, "error container"),
        (scheme.surface, "surface"),
        (scheme.surface_container_low, "surface container low"),
        (scheme.surface_container, "surface container"),
        (scheme.surface_container_high, "surface container high"),
        (
            scheme.surface_container_highest,
            "surface container highest",
        ),
    ];
    for row in swatches.chunks(5) {
        let cells = row
            .iter()
            .map(|(color, label)| swatch_cell(scheme, *color, label))
            .collect();
        v.push(any(Row(cells)));
    }

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(text("Type scale").size(20.0).color(scheme.on_surface)));
    let scale = &theme.type_scale;
    let tokens = [
        ("Display large", scale.display_large.clone()),
        ("Display medium", scale.display_medium.clone()),
        ("Display small", scale.display_small.clone()),
        ("Headline large", scale.headline_large.clone()),
        ("Headline medium", scale.headline_medium.clone()),
        ("Headline small", scale.headline_small.clone()),
        ("Title large", scale.title_large.clone()),
        ("Title medium", scale.title_medium.clone()),
        ("Title small", scale.title_small.clone()),
        ("Body large", scale.body_large.clone()),
        ("Body medium", scale.body_medium.clone()),
        ("Body small", scale.body_small.clone()),
        ("Label large", scale.label_large.clone()),
        ("Label medium", scale.label_medium.clone()),
        ("Label small", scale.label_small.clone()),
    ];
    for (label, style) in tokens {
        v.push(any(text(label).style(style).color(scheme.on_surface)));
    }

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(text("Elevation").size(20.0).color(scheme.on_surface)));
    let elevation = &theme.elevation;
    let cards = vec![
        elevation_cell(theme, scheme, elevation.level1, "level 1"),
        elevation_cell(theme, scheme, elevation.level2, "level 2"),
        elevation_cell(theme, scheme, elevation.level3, "level 3"),
        elevation_cell(theme, scheme, elevation.level4, "level 4"),
    ];
    v.push(any(Row(cards)));

    v
}
