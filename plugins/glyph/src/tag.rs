//! [`tag`]/[`TagView`]: a neutral, removable chip (the Glyph design
//! system's `.tag`/`.tag button` rules, retrieved
//! 2026-07-21) — a raised-surface pill carrying a label and an optional
//! trailing "×" remove affordance.
//!
//! # Color resolution
//!
//! `ColorScheme` has no dedicated "tag" role; this widget reuses the same
//! neutral mapping [`crate::badge`]'s `Neutral` variant uses
//! (`surface_container_high`/`on_surface_variant`/`outline` — Glyph's own
//! `bg-raised`/`fg-muted`/`border-bright`, see [`crate::tokens::color`]),
//! falling back to a literal Glyph **dark** constant per role with no theme
//! threaded.
//!
//! The remove glyph's source color (`--fg-dim`) has **no** `ColorScheme` role
//! at all — [`crate::tokens::color`]'s own module docs already document
//! this exact gap ("`fg-dim` … has no distinct `ColorScheme` role
//! (`on_surface_variant` already carries `fg-muted`)"), so the themed path
//! reuses `on_surface_variant` too (a documented simplification: the label and
//! remove glyph share one role, losing the source's subtle fg-muted/fg-dim
//! distinction). The **unthemed** fallback keeps the literal, more precise
//! `fg-dim` hex instead, since it costs nothing extra to be exact there.
//!
//! # Why this widget doesn't nest a `Text` child
//!
//! See [`crate::badge`]'s module docs — the same reasoning applies
//! here (a per-widget dynamic color the four fixed `ThemeTextColor` roles
//! don't cover), so `TagWidget` shapes and paints its own label/remove-glyph
//! runs directly via `frust_text`.
//!
//! Both runs read their family at layout from the live theme's `labelMedium`
//! type-scale role (IBM Plex Mono under Glyph's own scale), falling back to
//! Glyph's IBM Plex Mono stack unthemed, so a theme swap reshapes them (see
//! [`crate::badge`]'s Typeface section).

use std::rc::Rc;

use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::press::presses;

/// Corner-rounding tolerance for the tag's hairline border (matches
/// `crate::badge`'s constant of the same name).
const PATH_TOLERANCE: f64 = 0.1;

/// Horizontal padding, in logical px (`.tag{padding:5px 10px}`).
const TAG_PAD_X: f64 = 10.0;
/// Vertical padding, in logical px.
const TAG_PAD_Y: f64 = 5.0;
/// Gap between the label and the remove glyph, in logical px
/// (`.tag{gap:6px}`).
const TAG_GAP: f64 = 6.0;
/// Label + remove-glyph font size, in logical px (`.tag{font-size:11px}`,
/// `.tag button{font-size:11px}`).
const TAG_FONT_SIZE: f32 = 11.0;
/// Border width, in logical px (`.tag{border:1px solid var(--border-bright)}`).
const TAG_BORDER_WIDTH: f64 = 1.0;
/// Extra hit-test padding around the small remove glyph, in logical px — a
/// **community-approximate** usability floor (the CSS source has no touch
/// target concept), matching the general spirit of the platform's minimum
/// touch-target guidance without pinning to a specific published constant.
const REMOVE_HIT_SLOP: f64 = 4.0;

// ---- Unthemed fallback constants (Glyph **dark** values; see module docs) --

const TAG_BG: Color = Color::from_rgb8(0x1e, 0x23, 0x30); // bg-raised
const TAG_FG: Color = Color::from_rgb8(0xa3, 0x9c, 0x88); // fg-muted
const TAG_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // border-bright
const TAG_REMOVE_FG: Color = Color::from_rgb8(0x6b, 0x62, 0x52); // fg-dim (exact; see module docs)

/// A view-held, typed remove callback (erased on build).
type OnRemove<State> = Rc<dyn Fn(&mut State)>;

/// A declarative removable tag. See the [module docs](self).
pub struct TagView<State: 'static> {
    label: String,
    on_remove: Option<OnRemove<State>>,
}

/// Create a tag labelled `label` with no remove affordance; chain
/// [`TagView::on_remove`] to add one.
pub fn tag<State: 'static>(label: impl Into<String>) -> TagView<State> {
    TagView {
        label: label.into(),
        on_remove: None,
    }
}

impl<State: 'static> TagView<State> {
    /// Add a trailing "×" remove affordance that runs `on_remove` against the
    /// app state when released inside its bounds (fire-on-up-inside, like
    /// every other interactive widget in this crate — see
    /// `docs/CODE_STANDARDS.md`'s Interaction Semantics).
    pub fn on_remove<F: Fn(&mut State) + 'static>(mut self, on_remove: F) -> Self {
        self.on_remove = Some(Rc::new(on_remove));
        self
    }
}

/// A minimal retained text run — see `crate::badge`'s `GlyphLabel` for
/// the full shape/rationale (duplicated here rather than shared, matching
/// this crate's existing per-file small-helper convention, e.g. the
/// `cupertino::*`'s repeated `with_alpha`).
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

/// The retained widget for a [`TagView`].
pub struct TagWidget {
    label: GlyphLabel,
    label_text: String,
    label_size: Size,
    remove_glyph: GlyphLabel,
    removable: bool,
    remove_size: Size,
    /// The remove glyph's local (pod-relative) origin, computed at layout.
    remove_origin: Point,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` inside the remove hit-region, cleared on `Up`/`Cancel`.
    captured: bool,
    on_remove: Option<frust::authoring::ErasedCallback>,
}

impl<State: 'static> View<State> for TagView<State> {
    type Element = TagWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TagWidget {
        TagWidget {
            label: GlyphLabel::new(self.label.clone()),
            label_text: self.label.clone(),
            label_size: Size::ZERO,
            remove_glyph: GlyphLabel::new("\u{d7}"), // ×
            removable: self.on_remove.is_some(),
            remove_size: Size::ZERO,
            remove_origin: Point::ZERO,
            pressed: false,
            captured: false,
            on_remove: self
                .on_remove
                .as_ref()
                .map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TagWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_remove = self
            .on_remove
            .as_ref()
            .map(frust::authoring::erase_callback);
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label.set_content(self.label.clone());
            element.label_text = self.label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let removable_now = self.on_remove.is_some();
        if element.removable != removable_now {
            element.removable = removable_now;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// Glyph's UI face stack (IBM Plex Mono): both runs' unthemed family.
fn ui_face() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

/// The label's style: weight/size are Glyph-authored constants, the family is
/// the theme's `labelMedium` role; `color` varies.
fn tag_label_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.label_medium.family.clone()),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(TAG_FONT_SIZE, color)
    }
}

/// The remove glyph's style (`.tag button{font-family:var(--font-mono);
/// font-size:11px}`), family from the same `labelMedium` role as the label.
fn remove_glyph_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.label_medium.family.clone()),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(TAG_FONT_SIZE, color)
    }
}

/// Resolve `(background, label fg, border, remove-glyph fg)` — see the
/// module docs for the fg-dim/on_surface_variant simplification.
fn resolve_tag_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.surface_container_high,
                scheme.on_surface_variant,
                scheme.outline,
                scheme.on_surface_variant,
            )
        }
        None => (TAG_BG, TAG_FG, TAG_BORDER, TAG_REMOVE_FG),
    }
}

/// The tag's corner radius: themed `shape.small` resolved against the box
/// (`--radius-sm`, 6px). Unthemed: `TAG_RADIUS_FALLBACK` exactly.
const TAG_RADIUS_FALLBACK: f64 = 6.0;

fn resolve_tag_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.small, size.width, size.height),
        None => TAG_RADIUS_FALLBACK,
    }
}

fn inside(pos: Point, rect: Rect) -> bool {
    rect.contains(pos)
}

impl Widget for TagWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve both styles up front so the immutable theme borrow ends
        // before the `&mut ctx` shaping calls below.
        let theme = Theme::from_layout_ctx(ctx);
        let (_, label_fg, _, remove_fg) = resolve_tag_colors(theme);
        let label_style = tag_label_style(theme, label_fg);
        let remove_style = remove_glyph_style(theme, remove_fg);

        let label_size = self.label.layout(ctx, &label_style, None);
        self.label_size = label_size;

        let mut width = TAG_PAD_X * 2.0 + label_size.width;
        let mut content_height = label_size.height;

        if self.removable {
            let remove_size = self.remove_glyph.layout(ctx, &remove_style, None);
            self.remove_size = remove_size;
            width += TAG_GAP + remove_size.width;
            content_height = content_height.max(remove_size.height);
        } else {
            self.remove_size = Size::ZERO;
        }

        let height = content_height + TAG_PAD_Y * 2.0;
        let size = bc.constrain(Size::new(width, height));

        if self.removable {
            let x = size.width - TAG_PAD_X - self.remove_size.width;
            let y = (size.height - self.remove_size.height) / 2.0;
            self.remove_origin = Point::new(x, y);
        } else {
            self.remove_origin = Point::ZERO;
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, _, border, _) = resolve_tag_colors(theme);
        let radius = resolve_tag_radius(theme, ctx.size());
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), radius, bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, ctx.size()), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(ctx.origin(), &path, TAG_BORDER_WIDTH, &Brush::Solid(border));

        let label_y = (ctx.size().height - self.label_size.height) / 2.0;
        self.label
            .paint(ctx.origin() + Vec2::new(TAG_PAD_X, label_y), scene);

        if self.removable {
            self.remove_glyph
                .paint(ctx.origin() + self.remove_origin.to_vec2(), scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.removable {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let hit_rect = Rect::from_origin_size(self.remove_origin, self.remove_size)
            .inflate(REMOVE_HIT_SLOP, REMOVE_HIT_SLOP);
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                if !inside(p.position, hit_rect) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, hit_rect);
                self.pressed = inside_now;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, hit_rect)
                    && let Some(on_remove) = &mut self.on_remove
                {
                    on_remove(ctx);
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Label, |node| {
            node.set_label(self.label_text.as_str());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct RemoveState {
        removed: u32,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _path: &kurbo::BezPath, _width: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn build_removable() -> TagWidget {
        let view = tag::<RemoveState>("rust").on_remove(|s: &mut RemoveState| s.removed += 1);
        let mut counter = 0u64;
        View::<RemoveState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn build_plain() -> TagWidget {
        let view: TagView<()> = tag("rust");
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(widget: &mut TagWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)))
    }

    fn paint(widget: &mut TagWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    fn dispatch(widget: &mut TagWidget, state: &mut RemoveState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let size = Size::new(100.0, 32.0);
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        let mut w = build_removable();
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, TAG_BG);
        assert_eq!(rec.strokes[0], TAG_BORDER);
        // label then remove glyph, in that order.
        assert_eq!(rec.glyph_colors[0], TAG_FG);
        assert_eq!(rec.glyph_colors[1], TAG_REMOVE_FG);
    }

    #[test]
    fn glyph_dark_theme_resolves_neutral_colors() {
        let theme = crate::baseline();
        let mut w = build_removable();
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.surface_container_high);
        assert_eq!(rec.strokes[0], scheme.outline);
        assert_eq!(rec.glyph_colors[0], scheme.on_surface_variant);
    }

    #[test]
    fn glyph_light_theme_resolves_neutral_colors() {
        let theme = crate::baseline().with_brightness(frust::Brightness::Light);
        let mut w = build_removable();
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.surface_container_high);
    }

    #[test]
    fn non_removable_tag_paints_no_remove_glyph() {
        let mut w = build_plain();
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.glyph_colors.len(), 1); // label only
    }

    #[test]
    fn up_inside_remove_glyph_fires_once() {
        let mut w = build_removable();
        layout(&mut w, None);
        let mut state = RemoveState::default();
        let hit =
            w.remove_origin + Vec2::new(w.remove_size.width / 2.0, w.remove_size.height / 2.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, hit.x, hit.y));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, hit.x, hit.y));
        assert_eq!(state.removed, 1);
    }

    #[test]
    fn down_outside_remove_glyph_is_ignored() {
        let mut w = build_removable();
        layout(&mut w, None);
        let mut state = RemoveState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 2.0, 2.0));
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 2.0, 2.0));
        assert_eq!(state.removed, 0);
    }

    #[test]
    fn cancel_clears_pressed_state_without_firing() {
        let mut w = build_removable();
        layout(&mut w, None);
        let mut state = RemoveState::default();
        let hit =
            w.remove_origin + Vec2::new(w.remove_size.width / 2.0, w.remove_size.height / 2.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, hit.x, hit.y));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, hit.x, hit.y));
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, hit.x, hit.y));
        assert_eq!(state.removed, 0);
    }

    #[test]
    fn semantics_reports_label_role_and_text() {
        fn logic(_s: &mut ()) -> TagView<()> {
            tag("rust")
        }
        let mut root: frust_core::RenderRoot<(), TagView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Label)
            .expect("tag contributes a Role::Label node");
        assert_eq!(node.label(), Some("rust"));
    }

    // ---- Typeface: both runs follow their type-scale role -----------------

    #[cfg(feature = "bundled-fonts")]
    fn removable(_: &mut ()) -> TagView<()> {
        tag("rust").on_remove(|_s: &mut ()| {})
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_paint_in_their_role_face_under_the_glyph_theme() {
        use crate::badge::typeface_probe::{Face, painted_faces};
        let faces = painted_faces(removable, crate::baseline(), Size::new(200.0, 100.0));
        // Label, remove glyph.
        assert_eq!(faces, [Face::PlexMono; 2]);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_follow_a_live_theme_family_swap() {
        use crate::badge::typeface_probe::{Face, faces_across_a_live_swap};
        let (before, after) = faces_across_a_live_swap(removable, Size::new(200.0, 100.0));
        assert_eq!(before, [Face::PlexMono; 2]);
        assert_eq!(after, [Face::SpaceMono; 2]);
    }
}
