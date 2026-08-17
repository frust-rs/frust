//! [`segmented_control`]/[`SegmentedControlView`]: a pill container of
//! mutually-exclusive segments — the active one gets a raised surface fill and
//! accent text (the Glyph design system's `.segmented`/`.segmented
//! button`/`.segmented button.active` rules, retrieved 2026-07-21).
//!
//! # Controlled selection
//!
//! Like [`crate::tabs`] and `crate::material::navigation_bar`, this is
//! a **controlled** component: it reports the requested index through
//! `on_select(index)` and never mutates its own `selected`.
//!
//! # Per-brightness active treatment (the source's dark/light split)
//!
//! The source deliberately treats the active segment differently per
//! brightness ("segmented-control
//! fill gets a subtle shadow instead of a glow on light surfaces — glow reads
//! as haze on white, a shadow reads as lift"):
//!
//! - **Dark**: the active fill is `bg-overlay` (`surface_container_highest`),
//!   no shadow.
//! - **Light**: the active fill is `bg-surface` (`surface_container_lowest`,
//!   the lightest slot on the light baseline — the per-brightness structural
//!   inversion documented in [`crate::tokens`]), plus a subtle
//!   `0 1px 2px rgba(34,29,18,0.08)` ink drop shadow (`SEG_LIGHT_SHADOW_*`,
//!   an inline source token since no clean `Elevation` level matches this
//!   1px/2px micro-shadow — the shadow color resolves from the theme's warm-ink
//!   `shadow` role at [`SEG_LIGHT_SHADOW_ALPHA`]).
//!
//! Because the two brightnesses draw from *different* surface roles (and only
//! light carries a shadow), the active treatment branches on `theme.brightness`
//! rather than resolving one role for both — the one place a Glyph widget reads
//! brightness directly, mirroring the source.
//!
//! # Why this widget doesn't nest `Text` children
//!
//! See [`crate::badge`]'s module docs — the per-segment dynamic color
//! (accent vs muted) isn't one of `TextView`'s four fixed roles, so this widget
//! shapes/paints its own label glyph runs directly via `frust_text`.

use std::rc::Rc;

use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{Brightness, ShapeScale, Theme};
use kurbo::{Point, Rect, Size, Vec2};
use peniko::Color;

/// Container inner padding, logical px (`.segmented{padding:3px}`).
const SEG_PAD: f64 = 3.0;
/// Gap between segments, logical px (`.segmented{gap:2px}`).
const SEG_GAP: f64 = 2.0;
/// Segment horizontal padding, logical px (`.segmented button{padding:7px 14px}`).
const SEG_BTN_PAD_X: f64 = 14.0;
/// Segment vertical padding, logical px.
const SEG_BTN_PAD_Y: f64 = 7.0;
/// Label font size, logical px (`.segmented button{font-size:11px}`).
const SEG_FONT_SIZE: f32 = 11.0;
/// Container border width, logical px (`.segmented{border:1px}`).
const SEG_BORDER_W: f64 = 1.0;

/// The light-mode active-segment ink shadow (`box-shadow:0 1px 2px
/// rgba(34,29,18,0.08)`): a `blur_std_dev` of 2 (the CSS blur radius stored
/// directly, matching `frust_theme::elevation`'s v1 treatment) and an alpha of
/// 0.08 over the theme's warm-ink `shadow` role. Inline rather than an
/// `Elevation` level: no level matches this 1px/2px micro-shadow.
const SEG_LIGHT_SHADOW_BLUR: f64 = 2.0;
/// See [`SEG_LIGHT_SHADOW_BLUR`]. The unthemed fallback is the Glyph **dark**
/// baseline, which carries no active-segment shadow at all, so there is no
/// unthemed shadow color constant — the light ink shadow only ever appears via
/// a threaded light theme's warm-ink `shadow` role.
const SEG_LIGHT_SHADOW_ALPHA: f32 = 0.08;

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

const SEG_CONTAINER_BG: Color = Color::from_rgb8(0x1e, 0x23, 0x30); // bg-raised
const SEG_CONTAINER_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44); // border-bright
const SEG_ACTIVE_FILL_DARK: Color = Color::from_rgb8(0x27, 0x2d, 0x3d); // bg-overlay
const SEG_ACTIVE_TEXT: Color = Color::from_rgb8(0xff, 0xb6, 0x27); // amber
const SEG_INACTIVE_TEXT: Color = Color::from_rgb8(0x6b, 0x65, 0x56); // fg-dim

/// Return `color` with its alpha channel replaced (mirrors the same helper in
/// the sibling Glyph widgets).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved segmented-control palette.
struct SegColors {
    container_bg: Color,
    container_border: Color,
    active_fill: Color,
    active_text: Color,
    inactive_text: Color,
    /// `Some` only on the light baseline — the per-brightness ink lift shadow.
    active_shadow: Option<Color>,
}

/// Resolve the full palette — see the module docs for the per-brightness
/// active-segment split.
fn resolve_seg_colors(theme: Option<&Theme>) -> SegColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            let (active_fill, active_shadow) = match theme.brightness {
                Brightness::Light => (
                    scheme.surface_container_lowest,
                    Some(with_alpha(scheme.shadow, SEG_LIGHT_SHADOW_ALPHA)),
                ),
                Brightness::Dark => (scheme.surface_container_highest, None),
            };
            SegColors {
                container_bg: scheme.surface_container_high,
                container_border: scheme.outline,
                active_fill,
                active_text: scheme.primary,
                inactive_text: scheme.on_surface_variant,
                active_shadow,
            }
        }
        None => SegColors {
            container_bg: SEG_CONTAINER_BG,
            container_border: SEG_CONTAINER_BORDER,
            active_fill: SEG_ACTIVE_FILL_DARK,
            active_text: SEG_ACTIVE_TEXT,
            inactive_text: SEG_INACTIVE_TEXT,
            active_shadow: None,
        },
    }
}

/// Container radius (`--radius-sm`, 6px) — themed `shape.small`, else the
/// literal fallback.
fn container_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.small, size.width, size.height),
        None => 6.0,
    }
}

/// Active-segment radius (`.segmented button{border-radius:4px}`) — themed
/// `shape.extra_small`, else the literal fallback.
fn segment_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.extra_small, size.width, size.height),
        None => 4.0,
    }
}

/// A view-held, typed selection callback.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative segmented control. See the [module docs](self).
pub struct SegmentedControlView<State: 'static> {
    labels: Vec<String>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Create a segmented control over `labels`, with `selected` the current
/// (app-confirmed) index. Fires `on_select(state, index)` on a release inside a
/// segment — a **controlled** component.
pub fn segmented_control<State: 'static, F: Fn(&mut State, usize) + 'static>(
    labels: Vec<String>,
    selected: usize,
    on_select: F,
) -> SegmentedControlView<State> {
    SegmentedControlView {
        labels,
        selected,
        on_select: Rc::new(on_select),
    }
}

/// A minimal retained text run — see [`crate::badge`]'s `GlyphLabel`.
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

/// One retained segment.
struct Segment {
    label: GlyphLabel,
    text: String,
    /// Local x/width of the segment's fill rect (filled at layout).
    x: f64,
    width: f64,
    label_size: Size,
}

/// The label's fixed style.
fn seg_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(SEG_FONT_SIZE, color)
    }
}

/// The retained widget for a [`SegmentedControlView`].
pub struct SegmentedControlWidget {
    segments: Vec<Segment>,
    selected: usize,
    on_select: frust::authoring::ErasedArgCallback<usize>,
    height: f64,
    pressed: Option<usize>,
    captured: Option<usize>,
}

impl<State: 'static> View<State> for SegmentedControlView<State> {
    type Element = SegmentedControlWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SegmentedControlWidget {
        let segments = self
            .labels
            .iter()
            .map(|l| Segment {
                label: GlyphLabel::new(l.clone()),
                text: l.clone(),
                x: 0.0,
                width: 0.0,
                label_size: Size::ZERO,
            })
            .collect();
        SegmentedControlWidget {
            segments,
            selected: self.selected,
            on_select: frust::authoring::erase_callback_arg(&self.on_select),
            height: 0.0,
            pressed: None,
            captured: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SegmentedControlWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = frust::authoring::erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;
        if prev.labels.len() != self.labels.len() {
            element.segments = self
                .labels
                .iter()
                .map(|l| Segment {
                    label: GlyphLabel::new(l.clone()),
                    text: l.clone(),
                    x: 0.0,
                    width: 0.0,
                    label_size: Size::ZERO,
                })
                .collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (seg, label) in element.segments.iter_mut().zip(self.labels.iter()) {
                if &seg.text != label {
                    seg.label.set_content(label.clone());
                    seg.text = label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }
        if prev.selected != self.selected {
            element.selected = self.selected;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl SegmentedControlWidget {
    /// The segment index under a widget-local pointer position, if any.
    fn hit_index(&self, pos: Point) -> Option<usize> {
        self.segments.iter().position(|s| {
            Rect::from_origin_size(Point::new(s.x, 0.0), Size::new(s.width, self.height))
                .contains(pos)
        })
    }
}

impl Widget for SegmentedControlWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_seg_colors(theme);

        let mut x = SEG_PAD;
        let mut content_height = 0.0_f64;
        for (i, seg) in self.segments.iter_mut().enumerate() {
            let color = if i == self.selected {
                colors.active_text
            } else {
                colors.inactive_text
            };
            let style = seg_style(color);
            let label_size = seg.label.layout(ctx, &style, None);
            seg.label_size = label_size;
            seg.x = x;
            seg.width = label_size.width + SEG_BTN_PAD_X * 2.0;
            let seg_height = label_size.height + SEG_BTN_PAD_Y * 2.0;
            content_height = content_height.max(seg_height);
            x += seg.width + SEG_GAP;
        }
        let inner_width = if self.segments.is_empty() {
            0.0
        } else {
            x - SEG_GAP - SEG_PAD
        };
        let width = inner_width + SEG_PAD * 2.0;
        let height = content_height + SEG_PAD * 2.0;
        self.height = height;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_seg_colors(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        // Container: raised fill + hairline border.
        let radius = container_radius(theme, size);
        scene.fill_rounded_rect(origin, size, radius, colors.container_bg);
        let rr = kurbo::RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        scene.stroke_path(
            origin,
            &kurbo::Shape::to_path(&rr, 0.1),
            SEG_BORDER_W,
            &peniko::Brush::Solid(colors.container_border),
        );

        // Active segment fill (+ light-mode ink lift shadow beneath it).
        if let Some(seg) = self.segments.get(self.selected) {
            let seg_origin = origin + Vec2::new(seg.x, SEG_PAD);
            let seg_size = Size::new(seg.width, size.height - SEG_PAD * 2.0);
            let seg_radius = segment_radius(theme, seg_size);
            if let Some(shadow) = colors.active_shadow {
                scene.draw_shadow(
                    seg_origin + Vec2::new(0.0, 1.0),
                    seg_size,
                    seg_radius,
                    SEG_LIGHT_SHADOW_BLUR,
                    shadow,
                );
            }
            scene.fill_rounded_rect(seg_origin, seg_size, seg_radius, colors.active_fill);
        }

        // Labels (layout-baked color).
        for seg in &self.segments {
            let label_y = origin.y + (size.height - seg.label_size.height) / 2.0;
            let label_x = origin.x + seg.x + (seg.width - seg.label_size.width) / 2.0;
            seg.label.paint(Point::new(label_x, label_y), scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => match self.hit_index(p.position) {
                Some(i) => {
                    self.pressed = Some(i);
                    self.captured = Some(i);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                None => EventResult::Ignored,
            },
            PointerPhase::Move => {
                let Some(cap) = self.captured else {
                    return EventResult::Ignored;
                };
                self.pressed = (self.hit_index(p.position) == Some(cap)).then_some(cap);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(cap) = self.captured else {
                    return EventResult::Ignored;
                };
                if self.hit_index(p.position) == Some(cap) {
                    (self.on_select)(ctx, cap);
                }
                self.pressed = None;
                self.captured = None;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.is_none() {
                    return EventResult::Ignored;
                }
                self.pressed = None;
                self.captured = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for (i, seg) in self.segments.iter().enumerate() {
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(seg.text.as_str());
                        node.set_selected(i == self.selected);
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::PointerButton;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &peniko::Brush) {
            if let peniko::Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let peniko::Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn build(view: &SegmentedControlView<Vec<usize>>) -> SegmentedControlWidget {
        let mut counter = 0u64;
        View::<Vec<usize>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn labels() -> Vec<String> {
        vec!["grid".into(), "stack".into(), "zoom".into()]
    }

    fn layout(w: &mut SegmentedControlWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(800.0, 100.0)))
    }

    fn paint(w: &mut SegmentedControlWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut pctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn unthemed_paint_uses_dark_active_fill_and_no_shadow() {
        let view: SegmentedControlView<Vec<usize>> = segmented_control(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        // rrects[0] container, rrects[1] active fill.
        assert_eq!(rec.rrects[0].3, SEG_CONTAINER_BG);
        assert_eq!(rec.rrects[1].3, SEG_ACTIVE_FILL_DARK);
        assert_eq!(rec.strokes[0], SEG_CONTAINER_BORDER);
        assert!(rec.shadows.is_empty(), "no shadow in dark/unthemed");
        // Active label accent, others muted.
        assert_eq!(rec.glyph_colors[0], SEG_ACTIVE_TEXT);
        assert_eq!(rec.glyph_colors[1], SEG_INACTIVE_TEXT);
    }

    #[test]
    fn glyph_dark_theme_resolves_roles_without_shadow() {
        let theme = crate::baseline();
        let view: SegmentedControlView<Vec<usize>> = segmented_control(labels(), 1, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.surface_container_high);
        assert_eq!(rec.rrects[1].3, scheme.surface_container_highest);
        assert_eq!(rec.strokes[0], scheme.outline);
        assert!(rec.shadows.is_empty());
        // Segment 1 is active → accent.
        assert_eq!(rec.glyph_colors[1], scheme.primary);
    }

    #[test]
    fn glyph_light_theme_adds_an_ink_lift_shadow_under_the_active_segment() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let view: SegmentedControlView<Vec<usize>> = segmented_control(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        // Light active fill is the lightest surface slot, not the highest.
        assert_eq!(rec.rrects[1].3, scheme.surface_container_lowest);
        // Exactly one ink shadow, colored from the warm-ink `shadow` role.
        assert_eq!(rec.shadows.len(), 1);
        let (_, _, _, blur, color) = rec.shadows[0];
        assert_eq!(blur, SEG_LIGHT_SHADOW_BLUR);
        assert_eq!(color, with_alpha(scheme.shadow, SEG_LIGHT_SHADOW_ALPHA));
    }

    #[test]
    fn selection_is_controlled() {
        let view: SegmentedControlView<Vec<usize>> =
            segmented_control(labels(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, None);
        let mut log: Vec<usize> = Vec::new();
        let state_any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, w.height));
        let cx = w.segments[2].x + w.segments[2].width / 2.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, cx, w.height / 2.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, cx, w.height / 2.0));
        assert_eq!(log, vec![2]);
        assert_eq!(w.selected, 0);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_s: &mut ()) -> SegmentedControlView<()> {
            segmented_control(
                vec!["grid".into(), "stack".into(), "zoom".into()],
                2,
                |_s: &mut (), _i| {},
            )
        }
        let mut root: frust_core::RenderRoot<(), SegmentedControlView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let selected = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Tab && n.label() == Some("zoom"))
            .expect("the selected segment");
        assert_eq!(selected.1.is_selected(), Some(true));
    }
}
