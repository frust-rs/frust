//! [`empty_state`]/[`EmptyStateView`]: the "nothing here yet" placeholder — a
//! dashed-border panel with a large, faint centered glyph slot (a font char by
//! default, the Space Mono display face — or a vector icon, see below), a
//! title, a description, and an optional action child-view slot (the Glyph
//! design system's `.empty-state`).
//!
//! The title, description, and centered glyph are the widget's own text runs
//! (shaped/painted directly, like [`super::badge`]); the action slot is an
//! arbitrary child [`frust::View`] (typically a button), so its events
//! route through and its semantics is forwarded (`semantics_child`) — the
//! silent-drop rule.
//!
//! # Glyph slot: char vs. vector icon
//!
//! [`EmptyStateView::glyph`] sets a font char (the default is `∅`, U+2205
//! EMPTY SET), but a char outside the bundled Glyph fonts' coverage falls
//! back through the *platform's* system font — inconsistent, and on iOS
//! frequently invisible (e.g. `▣`/`⚙` are absent or render differently across
//! iOS/Android; see `crates/frust-widgets/src/icon.rs`'s module docs).
//! [`EmptyStateView::icon`] sets a deterministic vector path
//! instead — the same pixels on every platform, no font-fallback dependency.
//!
//! **Precedence: when both `.icon(..)` and `.glyph(..)` are set, the icon
//! wins.** The icon is the more specific, explicit vector value; this
//! mirrors [`frust::icon`]'s own "explicit builder value wins" convention
//! rather than introducing a new precedence rule.
//!
//! # Token resolution
//!
//! - **glyph slot** (char or icon) = `on_surface_variant` at
//!   [`GLYPH_FAINT_ALPHA`] (Glyph's "faintest" ghost ink — no opaque
//!   `ColorScheme` role exists for it, so a true alpha wash stands in).
//! - **title** = `on_surface`; **desc** = `on_surface_variant`.
//! - **dashed border** = `outline`.
//! - **families**, read at layout from the live type scale: the glyph char
//!   from `displaySmall` (Space Mono under Glyph's own scale), the title from
//!   `titleMedium` and the description from `bodyMedium` (both IBM Plex
//!   Mono), so a theme swap reshapes every run (see [`super::badge`]'s
//!   Typeface section).
//!
//! Unthemed, each falls back to the literal Glyph **dark** constant (each
//! family to the matching Glyph stack).
//!
//! # Dashed border
//!
//! [`frust::authoring::PaintScene`] has no dashed-stroke primitive, so the border is
//! drawn as a run of short [`frust::authoring::PaintScene::stroke_line`] segments
//! ([`DASH_LEN`] on, [`DASH_GAP`] off) around a straight-edged rectangle inset
//! half the stroke width — the documented approximation of the source's
//! `border:1px dashed`.

use frust::IconData;
use frust::Theme;
use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Affine, Point, Size, Vec2};
use peniko::{Brush, Color};

/// Content padding on all four edges, in logical px.
const EMPTY_PADDING: f64 = 24.0;
/// Gap below the big glyph char, in logical px.
const EMPTY_GLYPH_GAP: f64 = 12.0;
/// Gap below the title, in logical px.
const EMPTY_TITLE_GAP: f64 = 6.0;
/// Gap above the action slot, in logical px.
const EMPTY_ACTION_GAP: f64 = 16.0;
/// Border width, in logical px.
const EMPTY_BORDER_WIDTH: f64 = 1.0;
/// Dash on-length, in logical px.
const DASH_LEN: f64 = 6.0;
/// Dash off-length (gap), in logical px.
const DASH_GAP: f64 = 4.0;

/// Big centered glyph font size, in logical px.
const EMPTY_GLYPH_SIZE: f32 = 40.0;
/// Title font size, in logical px.
const EMPTY_TITLE_SIZE: f32 = 15.0;
/// Description font size, in logical px.
const EMPTY_DESC_SIZE: f32 = 12.5;
/// Prose line height for the description.
const EMPTY_DESC_LINE_HEIGHT: f32 = 1.5;
/// The faint alpha applied to the big glyph char.
const GLYPH_FAINT_ALPHA: f32 = 0.35;

// ---- Unthemed fallback constants (Glyph **dark** values) ---------------

const EMPTY_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // outline
const EMPTY_TITLE_FG: Color = Color::from_rgb8(0xf2, 0xea, 0xd9); // fg
const EMPTY_DESC_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted
const EMPTY_GLYPH_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted (washed by alpha)

/// The default centered glyph char (an empty-set sign).
const DEFAULT_GLYPH: &str = "\u{2205}"; // ∅

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A declarative empty-state panel. See the [module docs](self).
pub struct EmptyStateView<State: 'static> {
    glyph: String,
    icon: Option<IconData>,
    title: String,
    desc: String,
    action: Option<AnyView<State>>,
}

/// Create an empty-state panel with a `title` and `desc`. Chain
/// [`EmptyStateView::glyph`] to change the centered char (or
/// [`EmptyStateView::icon`] for a vector icon instead — see the
/// [module docs](self)'s precedence note) and [`EmptyStateView::action`] to
/// add an action slot.
pub fn empty_state<State: 'static>(
    title: impl Into<String>,
    desc: impl Into<String>,
) -> EmptyStateView<State> {
    EmptyStateView {
        glyph: DEFAULT_GLYPH.to_string(),
        icon: None,
        title: title.into(),
        desc: desc.into(),
        action: None,
    }
}

impl<State: 'static> EmptyStateView<State> {
    /// Override the centered glyph char. Ignored if [`EmptyStateView::icon`]
    /// is also set — see the [module docs](self)'s precedence note.
    pub fn glyph(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = glyph.into();
        self
    }

    /// Set a vector icon for the centered glyph slot, taking precedence over
    /// [`EmptyStateView::glyph`]'s char — see the [module docs](self)'s
    /// "Glyph slot: char vs. vector icon" section.
    pub fn icon(mut self, icon: impl Into<IconData>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set the action slot (rendered centered below the description).
    pub fn action<V: View<State>>(mut self, view: V) -> Self {
        self.action = Some(any(view));
        self
    }
}

/// A minimal retained text run — see [`super::badge`]'s `GlyphLabel`.
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
    laid_out_max_width: Option<f32>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
            laid_out_max_width: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        self.laid_out_max_width = max_width;
        size
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// The retained centered glyph slot: a shaped char run (the `.glyph()`
/// builder path) or a resolved vector icon (`.icon()`, wins when both are
/// set — see the [module docs](self)). Mirrors
/// [`crate::navbar`]'s identical `NavSlot` shape.
enum EmptyGlyphSlot {
    // Boxed: `GlyphLabel` carries a `TextLayout`/`TextStyle` pair that makes
    // it far larger than `IconData`'s handful of bytes — an unboxed variant
    // would size the whole enum (and every `EmptyStateWidget`) off the
    // bigger arm (`clippy::large_enum_variant`).
    Char(Box<GlyphLabel>),
    Icon(IconData),
}

impl EmptyGlyphSlot {
    fn new(icon: &Option<IconData>, glyph: &str) -> Self {
        match icon {
            Some(data) => EmptyGlyphSlot::Icon(data.clone()),
            None => EmptyGlyphSlot::Char(Box::new(GlyphLabel::new(glyph))),
        }
    }

    /// Whether `self` (the retained slot) still matches the declarative
    /// `(icon, glyph)` pair a rebuild is diffing against — a cheap
    /// short-circuit so an unchanged char/icon skips rebuilding the slot
    /// (and, for the char path, keeps its cached shaped layout).
    fn matches(&self, icon: &Option<IconData>, glyph: &str) -> bool {
        match (self, icon) {
            (EmptyGlyphSlot::Icon(current), Some(next)) => current.same(next),
            (EmptyGlyphSlot::Char(label), None) => label.content == glyph,
            _ => false,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        match self {
            EmptyGlyphSlot::Char(label) => label.layout(ctx, style, None),
            // A vector icon occupies the same square box the glyph char's
            // face size defines — no text shaping involved.
            EmptyGlyphSlot::Icon(_) => Size::new(EMPTY_GLYPH_SIZE as f64, EMPTY_GLYPH_SIZE as f64),
        }
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        match self {
            EmptyGlyphSlot::Char(label) => label.paint(origin, scene),
            EmptyGlyphSlot::Icon(data) => {
                let (path, design) = data.resolve();
                let scale = if design > 0.0 {
                    EMPTY_GLYPH_SIZE as f64 / design
                } else {
                    1.0
                };
                let scaled = Affine::scale(scale) * path;
                scene.fill_path(origin, &scaled, &Brush::Solid(color));
            }
        }
    }
}

/// The retained widget for an [`EmptyStateView`].
pub struct EmptyStateWidget<State: 'static> {
    glyph: EmptyGlyphSlot,
    glyph_size: Size,
    title: GlyphLabel,
    title_text: String,
    title_size: Size,
    desc: GlyphLabel,
    desc_text: String,
    desc_size: Size,
    action: Option<ChildPod>,
    action_size: Size,
    _state: std::marker::PhantomData<State>,
}

impl<State: 'static> View<State> for EmptyStateView<State> {
    type Element = EmptyStateWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> EmptyStateWidget<State> {
        EmptyStateWidget {
            glyph: EmptyGlyphSlot::new(&self.icon, &self.glyph),
            glyph_size: Size::ZERO,
            title: GlyphLabel::new(self.title.clone()),
            title_text: self.title.clone(),
            title_size: Size::ZERO,
            desc: GlyphLabel::new(self.desc.clone()),
            desc_text: self.desc.clone(),
            desc_size: Size::ZERO,
            action: self
                .action
                .as_ref()
                .map(|v| frust::authoring::build_child(v, ctx)),
            action_size: Size::ZERO,
            _state: std::marker::PhantomData,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut EmptyStateWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if !element.glyph.matches(&self.icon, &self.glyph) {
            element.glyph = EmptyGlyphSlot::new(&self.icon, &self.glyph);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.title != self.title {
            element.title.set_content(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.desc != self.desc {
            element.desc.set_content(self.desc.clone());
            element.desc_text = self.desc.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        match (&prev.action, &self.action, element.action.as_mut()) {
            (Some(p), Some(n), Some(pod)) => {
                flags |= frust::authoring::rebuild_child(p, n, pod, ctx)
            }
            (None, Some(n), _) => {
                element.action = Some(frust::authoring::build_child(n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None, Some(_)) => {
                if let Some(mut pod) = element.action.take() {
                    frust::authoring::teardown_child(p, &mut pod, ctx);
                }
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }
        flags
    }

    fn teardown(&self, element: &mut EmptyStateWidget<State>, ctx: &mut BuildCtx<'_>) {
        if let (Some(v), Some(pod)) = (self.action.as_ref(), element.action.as_mut()) {
            frust::authoring::teardown_child(v, pod, ctx);
        }
    }
}

/// Glyph's display face stack (Space Mono): the glyph char's unthemed
/// family.
fn display_face() -> FontFamily {
    FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace)
}

/// Glyph's UI face stack (IBM Plex Mono): the title's and description's
/// unthemed family.
fn ui_face() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

/// The glyph char's style; family from the theme's `displaySmall` role.
fn glyph_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(display_face, |t| t.type_scale.display_small.family.clone()),
        weight: FontWeight::BOLD,
        ..TextStyle::new(EMPTY_GLYPH_SIZE, color)
    }
}

/// The title's style; family from the theme's `titleMedium` role.
fn title_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.title_medium.family.clone()),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(EMPTY_TITLE_SIZE, color)
    }
}

/// The description's style; family from the theme's `bodyMedium` role.
fn desc_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.body_medium.family.clone()),
        weight: FontWeight::REGULAR,
        line_height: LineHeight::FontSizeRelative(EMPTY_DESC_LINE_HEIGHT),
        ..TextStyle::new(EMPTY_DESC_SIZE, color)
    }
}

/// Resolve `(border, glyph, title, desc)` colors.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.outline,
                with_alpha(s.on_surface_variant, GLYPH_FAINT_ALPHA),
                s.on_surface,
                s.on_surface_variant,
            )
        }
        None => (
            EMPTY_BORDER,
            with_alpha(EMPTY_GLYPH_FG, GLYPH_FAINT_ALPHA),
            EMPTY_TITLE_FG,
            EMPTY_DESC_FG,
        ),
    }
}

/// Stroke a dashed straight-edged rectangle border along the perimeter of the
/// box at `origin`/`size` (see the module docs' Dashed border note).
fn stroke_dashed_rect(scene: &mut dyn PaintScene, origin: Point, size: Size, color: Color) {
    let half = EMPTY_BORDER_WIDTH / 2.0;
    let x0 = origin.x + half;
    let y0 = origin.y + half;
    let x1 = origin.x + size.width - half;
    let y1 = origin.y + size.height - half;
    let step = DASH_LEN + DASH_GAP;

    // Horizontal edges (top y0, bottom y1).
    let mut x = x0;
    while x < x1 {
        let end = (x + DASH_LEN).min(x1);
        scene.stroke_line(
            Point::new(x, y0),
            Point::new(end, y0),
            EMPTY_BORDER_WIDTH,
            color,
        );
        scene.stroke_line(
            Point::new(x, y1),
            Point::new(end, y1),
            EMPTY_BORDER_WIDTH,
            color,
        );
        x += step;
    }
    // Vertical edges (left x0, right x1).
    let mut y = y0;
    while y < y1 {
        let end = (y + DASH_LEN).min(y1);
        scene.stroke_line(
            Point::new(x0, y),
            Point::new(x0, end),
            EMPTY_BORDER_WIDTH,
            color,
        );
        scene.stroke_line(
            Point::new(x1, y),
            Point::new(x1, end),
            EMPTY_BORDER_WIDTH,
            color,
        );
        y += step;
    }
}

impl<State: 'static> Widget for EmptyStateWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve every style up front so the immutable theme borrow ends
        // before the `&mut ctx` shaping calls below.
        let theme = Theme::from_layout_ctx(ctx);
        let (_, glyph_c, title_c, desc_c) = resolve_colors(theme);
        let (glyph_style, title_style, desc_style) = (
            glyph_style(theme, glyph_c),
            title_style(theme, title_c),
            desc_style(theme, desc_c),
        );

        let inner_max_w = if bc.max().width.is_finite() {
            Some((bc.max().width - EMPTY_PADDING * 2.0).max(0.0) as f32)
        } else {
            None
        };

        self.glyph_size = self.glyph.layout(ctx, &glyph_style);
        self.title_size = self.title.layout(ctx, &title_style, inner_max_w);
        self.desc_size = self.desc.layout(ctx, &desc_style, inner_max_w);

        let mut content_h = self.glyph_size.height
            + EMPTY_GLYPH_GAP
            + self.title_size.height
            + EMPTY_TITLE_GAP
            + self.desc_size.height;
        let mut content_w = self
            .glyph_size
            .width
            .max(self.title_size.width)
            .max(self.desc_size.width);

        if let Some(pod) = self.action.as_mut() {
            let action_bc = BoxConstraints::new(
                Size::ZERO,
                Size::new(
                    inner_max_w.map(|w| w as f64).unwrap_or(f64::INFINITY),
                    f64::INFINITY,
                ),
            );
            self.action_size = pod.layout_child(ctx, &action_bc);
            content_h += EMPTY_ACTION_GAP + self.action_size.height;
            content_w = content_w.max(self.action_size.width);
        } else {
            self.action_size = Size::ZERO;
        }

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            content_w + EMPTY_PADDING * 2.0
        };
        let height = content_h + EMPTY_PADDING * 2.0;
        let size = bc.constrain(Size::new(width, height));

        // Center the action slot horizontally now that the final width is known.
        if let Some(pod) = self.action.as_mut() {
            let ax = (size.width - self.action_size.width) / 2.0;
            let ay = size.height - EMPTY_PADDING - self.action_size.height;
            pod.set_origin(Point::new(ax, ay));
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (border, glyph_c, _, _) = resolve_colors(theme);
        let o = ctx.origin();
        let size = ctx.size();

        stroke_dashed_rect(scene, o, size, border);

        let center_x = size.width / 2.0;
        let mut y = EMPTY_PADDING;
        self.glyph.paint(
            o + Vec2::new(center_x - self.glyph_size.width / 2.0, y),
            glyph_c,
            scene,
        );
        y += self.glyph_size.height + EMPTY_GLYPH_GAP;
        self.title.paint(
            o + Vec2::new(center_x - self.title_size.width / 2.0, y),
            scene,
        );
        y += self.title_size.height + EMPTY_TITLE_GAP;
        self.desc.paint(
            o + Vec2::new(center_x - self.desc_size.width / 2.0, y),
            scene,
        );

        if let Some(pod) = self.action.as_mut() {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(pod) = self.action.as_mut() {
            return frust::authoring::route_event_single(pod, ctx, event);
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::GenericContainer,
            |node| {
                node.set_label(self.title_text.as_str());
                node.set_description(self.desc_text.as_str());
            },
            |ctx| {
                if let Some(pod) = self.action.as_ref() {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(action);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        lines: Vec<(Point, Point, Color)>,
        glyph_colors: Vec<Color>,
        path_fills: Vec<(Point, kurbo::Rect, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_line(&mut self, p0: Point, p1: Point, _w: f64, color: Color) {
            self.lines.push((p0, p1, color));
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let peniko::Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
        fn fill_path(&mut self, origin: Point, path: &kurbo::BezPath, brush: &Brush) {
            use kurbo::Shape;
            if let Brush::Solid(c) = brush {
                self.path_fills.push((origin, path.bounding_box(), *c));
            }
        }
    }

    fn build<S: 'static>(view: &EmptyStateView<S>) -> EmptyStateWidget<S> {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint<S: 'static>(
        widget: &mut EmptyStateWidget<S>,
        theme: Option<&Theme>,
    ) -> Recorder {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_paints_dashed_border_and_faint_glyph() {
        let view: EmptyStateView<()> = empty_state("Nothing here", "Add something to begin");
        let mut w = build(&view);
        let rec = layout_and_paint(&mut w, None);
        // Multiple dash segments (not a single stroke) → dashed border.
        assert!(rec.lines.len() > 4, "dashed border emits many segments");
        assert!(rec.lines.iter().all(|(_, _, c)| *c == EMPTY_BORDER));
        // The big glyph run is the faint-alpha wash.
        assert_eq!(
            rec.glyph_colors[0],
            with_alpha(EMPTY_GLYPH_FG, GLYPH_FAINT_ALPHA)
        );
    }

    #[test]
    fn glyph_dark_and_light_borders_differ() {
        let dark = crate::baseline();
        let light = dark.clone().with_brightness(Brightness::Light);
        let view: EmptyStateView<()> = empty_state("t", "d");
        let mut wd = build(&view);
        let mut wl = build(&view);
        let rec_d = layout_and_paint(&mut wd, Some(&dark));
        let rec_l = layout_and_paint(&mut wl, Some(&light));
        assert_eq!(rec_d.lines[0].2, dark.scheme().outline);
        assert_eq!(rec_l.lines[0].2, light.scheme().outline);
        assert_ne!(dark.scheme().outline, light.scheme().outline);
    }

    #[test]
    fn default_glyph_is_the_empty_set_sign() {
        let view: EmptyStateView<()> = empty_state("t", "d");
        let w = build(&view);
        match &w.glyph {
            EmptyGlyphSlot::Char(label) => assert_eq!(label.content, DEFAULT_GLYPH),
            EmptyGlyphSlot::Icon(_) => panic!("default slot should be the char face, not an icon"),
        }
    }

    /// A 10×10-design filled diamond, mirroring
    /// `crate::navbar::tests::diamond_icon` — a known bounding box so
    /// the icon-vs-char slot painted-fill assertions are deterministic.
    fn diamond_icon() -> IconData {
        let mut p = kurbo::BezPath::new();
        p.move_to((5.0, 0.0));
        p.line_to((10.0, 5.0));
        p.line_to((5.0, 10.0));
        p.line_to((0.0, 5.0));
        p.close_path();
        IconData::from_path(p, 10.0)
    }

    #[test]
    fn icon_slot_paints_a_filled_path_not_a_glyph_run() {
        let view: EmptyStateView<()> =
            empty_state("Nothing here", "Add something to begin").icon(diamond_icon());
        let mut w = build(&view);
        let rec = layout_and_paint(&mut w, None);
        // The vector path is actually painted — a real fill_path call with a
        // non-degenerate bounding box scaled into the glyph box, not merely
        // that the builder accepted a value.
        assert_eq!(rec.path_fills.len(), 1, "one fill_path for the icon slot");
        let (_, bbox, color) = rec.path_fills[0];
        assert!(
            (bbox.width() - EMPTY_GLYPH_SIZE as f64).abs() < 1e-6,
            "icon scaled to the glyph box width"
        );
        assert!((bbox.height() - EMPTY_GLYPH_SIZE as f64).abs() < 1e-6);
        assert_eq!(
            color,
            with_alpha(EMPTY_GLYPH_FG, GLYPH_FAINT_ALPHA),
            "icon fill uses the same faint-wash glyph ink as the char face"
        );
        // No text-shaped glyph run is emitted for the icon face — the char
        // path is not exercised at all when an icon is set. The only
        // remaining glyph runs are the title and description text.
        assert_eq!(
            rec.glyph_colors.len(),
            2,
            "title + desc runs only, no centered-glyph char run"
        );
    }

    #[test]
    fn icon_wins_over_glyph_char_when_both_are_set() {
        let view: EmptyStateView<()> = empty_state("t", "d").glyph("X").icon(diamond_icon());
        let w = build(&view);
        match &w.glyph {
            EmptyGlyphSlot::Icon(_) => {}
            EmptyGlyphSlot::Char(_) => panic!("icon must win over glyph when both are set"),
        }
    }

    #[test]
    fn action_slot_receives_press_events() {
        #[derive(Default)]
        struct Counter {
            presses: u32,
        }
        let view: EmptyStateView<Counter> =
            empty_state("Empty", "Add one")
                .action(frust::button::<Counter, _>("add", |s: &mut Counter| {
                    s.presses += 1
                }));
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 300.0)));

        let ao = w.action.as_ref().unwrap().origin();
        let mut state = Counter::default();
        let ev = |phase, x: f64, y: f64| {
            InputEvent::Pointer(frust_core::PointerEvent {
                phase,
                position: Point::new(x, y),
                button: frust_core::PointerButton::Primary,
            })
        };
        let dispatch = |w: &mut EmptyStateWidget<Counter>, s: &mut Counter, e: &InputEvent| {
            let sa: &mut dyn Any = s;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(400.0, 300.0));
            w.event(&mut ctx, e)
        };
        dispatch(
            &mut w,
            &mut state,
            &ev(frust_core::PointerPhase::Down, ao.x + 4.0, ao.y + 4.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(frust_core::PointerPhase::Up, ao.x + 4.0, ao.y + 4.0),
        );
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn semantics_forwards_action_and_carries_title_desc() {
        fn logic(_s: &mut ()) -> EmptyStateView<()> {
            empty_state("No results", "Try another search").action(frust::text("retry"))
        }
        let mut root: frust_core::RenderRoot<(), EmptyStateView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::GenericContainer)
            .expect("empty state contributes a Role::GenericContainer node");
        assert_eq!(node.label(), Some("No results"));
        assert_eq!(node.description(), Some("Try another search"));
        assert!(
            !node.children().is_empty(),
            "the action is a semantics child"
        );
    }

    // ---- Typeface: every run's family follows its type-scale role ---------

    /// An empty state with an ASCII glyph char (inside both bundled families'
    /// coverage, unlike the default `∅`) and no action slot, so every painted
    /// run is one of the widget's own.
    #[cfg(feature = "bundled-fonts")]
    fn ascii_glyph(_: &mut ()) -> EmptyStateView<()> {
        empty_state("No results", "Try another search").glyph("?")
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_paint_in_their_role_faces_under_the_glyph_theme() {
        use crate::badge::typeface_probe::{Face, painted_faces};
        let faces = painted_faces(ascii_glyph, crate::baseline(), Size::new(300.0, 300.0));
        // Glyph char, title, description.
        assert_eq!(faces, [Face::SpaceMono, Face::PlexMono, Face::PlexMono]);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_follow_a_live_theme_family_swap() {
        use crate::badge::typeface_probe::{Face, faces_across_a_live_swap};
        let (before, after) = faces_across_a_live_swap(ascii_glyph, Size::new(300.0, 300.0));
        assert_eq!(before, [Face::SpaceMono, Face::PlexMono, Face::PlexMono]);
        assert_eq!(after, [Face::PlexMono, Face::SpaceMono, Face::SpaceMono]);
    }
}
