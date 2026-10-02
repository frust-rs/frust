//! Ports shadcn/ui's **Breadcrumb** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/breadcrumb.tsx`.
//!
//! [`breadcrumb_list`] is the row (`flex items-center gap-1.5`,
//! `text-sm text-muted-foreground`); [`breadcrumb`] is the same row with an
//! extra `Role::Navigation` semantics wrapper (`<nav aria-label="breadcrumb">`
//! carries no visual style of its own). [`breadcrumb_item`] is a pure
//! identity pass-through — the source's `inline-flex items-center gap-1.5`
//! wrapper has nothing to add once its child is already laid out in the
//! list's own row, so it is named here for fidelity but does no work.
//! [`breadcrumb_link`] is this catalog's breadcrumb-hover reference: it
//! claims hover from its own uncaptured `Move` arm, latches the hit test with
//! a changed-gated redraw, and self-corrects from `PaintCtx::is_hovered()` —
//! all three parts of the documented hover contract
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics) — recoloring
//! `on_surface_variant` → `on_surface` on hover by overriding each shaped
//! glyph run's brush at paint time rather than re-shaping. [`breadcrumb_page`]
//! is static `on_surface` text with no link semantics (the current page).
//! [`breadcrumb_separator`] draws a `size-3.5` chevron (the default) or hosts
//! a caller's own icon; [`breadcrumb_ellipsis`] is a decorative `size-9`
//! three-dot glyph.
//!
//! # Deviations
//!
//! - **No wrap.** The source's `flex-wrap` lets a long trail wrap to a second
//!   line; this port lays every part out on one row (deviation, matching
//!   [`crate::components::button_group`]'s single-row-only note).

use frust::authoring::text::TextStyle;
use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, CursorIcon, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, ThemeTextColor, ThemeTextType, View, Widget, any,
};
use frust::{Theme, text};
use kurbo::{Line, Shape};
use peniko::Color;

use crate::hit::{inside, presses};
use crate::style;
use crate::text::{LabelRun, SHAPING_INK, themed_family};

/// `gap-1.5` — the gap between breadcrumb parts.
const LIST_GAP: f64 = style::SPACING_UNIT * 1.5;
/// `size-3.5` — [`breadcrumb_separator`]'s default chevron box.
const SEPARATOR_SIZE: f64 = 14.0;
/// `size-9` — [`breadcrumb_ellipsis`]'s box (36px).
const ELLIPSIS_SIZE: f64 = 36.0;

/// Unthemed-fallback link ink (`text-muted-foreground`).
const FALLBACK_MUTED: Color = Color::from_rgb8(0x73, 0x73, 0x73);
/// Unthemed-fallback hovered link ink (`hover:text-foreground`).
const FALLBACK_FOREGROUND: Color = Color::from_rgb8(0x0A, 0x0A, 0x0A);

/// A declarative flex row of breadcrumb parts. See the [module docs](self).
pub struct BreadcrumbListView<State: 'static> {
    children: Vec<AnyView<State>>,
    is_nav: bool,
}

/// `BreadcrumbList`: the row itself, `gap-1.5`, `items-center`.
pub fn breadcrumb_list<State: 'static>(children: Vec<AnyView<State>>) -> BreadcrumbListView<State> {
    BreadcrumbListView {
        children,
        is_nav: false,
    }
}

/// `Breadcrumb`: the same row, with an `aria-label="breadcrumb"`
/// `Role::Navigation` semantics wrapper.
pub fn breadcrumb<State: 'static>(children: Vec<AnyView<State>>) -> BreadcrumbListView<State> {
    BreadcrumbListView {
        children,
        is_nav: true,
    }
}

/// `BreadcrumbItem`: an identity pass-through — see the [module docs](self).
pub fn breadcrumb_item<State: 'static, V: View<State>>(child: V) -> V {
    child
}

/// The retained widget for a [`BreadcrumbListView`].
pub struct BreadcrumbListWidget {
    children: Vec<ChildPod>,
    is_nav: bool,
}

impl<State: 'static> View<State> for BreadcrumbListView<State> {
    type Element = BreadcrumbListWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BreadcrumbListWidget {
        BreadcrumbListWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            is_nav: self.is_nav,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BreadcrumbListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = frust::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        );
        element.is_nav = self.is_nav;
        flags
    }

    fn teardown(&self, element: &mut BreadcrumbListWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for BreadcrumbListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = BoxConstraints::loose(Size::new(f64::INFINITY, bc.max().height));
        let mut sizes = Vec::with_capacity(self.children.len());
        let mut width = 0.0_f64;
        let mut height = 0.0_f64;
        for pod in self.children.iter_mut() {
            let s = pod.layout_child(ctx, &loose);
            sizes.push(s);
            height = height.max(s.height);
        }
        let mut x = 0.0_f64;
        for (pod, s) in self.children.iter_mut().zip(sizes.iter()) {
            pod.set_origin(Point::new(x, (height - s.height) / 2.0));
            x += s.width + LIST_GAP;
        }
        if !sizes.is_empty() {
            x -= LIST_GAP;
        }
        width = width.max(x);
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let visit = |ctx: &mut SemanticsCtx| {
            for pod in &self.children {
                pod.semantics_child(ctx);
            }
        };
        if self.is_nav {
            ctx.push_container(Role::Navigation, |_| {}, visit);
        } else {
            ctx.push_container(Role::GenericContainer, |_| {}, visit);
        }
    }

    frust::authoring::visit_children!(children);
}

// ---- BreadcrumbLink ---------------------------------------------------

/// The text style a link's label is shaped with: `text-sm` in the live theme's
/// `BodyMedium` family, at the sentinel ink every paint-time-recolored run uses
/// (the hover ink is applied per paint — see [`LabelRun`]).
fn link_style(theme: Option<&Theme>) -> TextStyle {
    themed_family(
        TextStyle::new(style::TEXT_SM as f32, SHAPING_INK),
        theme,
        ThemeTextType::BodyMedium,
    )
}

/// A view-held, typed click callback (erased on build).
type OnClick<State> = std::rc::Rc<dyn Fn(&mut State)>;

/// A declarative `BreadcrumbLink`. See the [module docs](self).
pub struct BreadcrumbLinkView<State: 'static> {
    label: String,
    on_click: Option<OnClick<State>>,
}

/// Create a link labelled `label`; call [`BreadcrumbLinkView::on_click`] to
/// make it fire on tap.
pub fn breadcrumb_link<State: 'static>(label: impl Into<String>) -> BreadcrumbLinkView<State> {
    BreadcrumbLinkView {
        label: label.into(),
        on_click: None,
    }
}

impl<State: 'static> BreadcrumbLinkView<State> {
    /// Fire `f` on release inside the link.
    pub fn on_click<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

/// The retained widget for a [`BreadcrumbLinkView`].
pub struct BreadcrumbLinkWidget {
    label: LabelRun,
    hovered: bool,
    captured: bool,
    on_click: Option<frust::authoring::ErasedCallback>,
}

impl<State: 'static> View<State> for BreadcrumbLinkView<State> {
    type Element = BreadcrumbLinkWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BreadcrumbLinkWidget {
        BreadcrumbLinkWidget {
            label: LabelRun::new(self.label.clone()),
            hovered: false,
            captured: false,
            on_click: self.on_click.as_ref().map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BreadcrumbLinkWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label {
            element.label = LabelRun::new(self.label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_click = self.on_click.as_ref().map(frust::authoring::erase_callback);
        flags
    }
}

fn resolve_link_ink(hovered: bool, theme: Option<&Theme>) -> Color {
    match (hovered, theme) {
        (true, Some(t)) => t.scheme().on_surface,
        (false, Some(t)) => t.scheme().on_surface_variant,
        (true, None) => FALLBACK_FOREGROUND,
        (false, None) => FALLBACK_MUTED,
    }
}

impl Widget for BreadcrumbLinkWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = link_style(Theme::from_layout_ctx(ctx));
        bc.constrain(self.label.layout(ctx, &style))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Self-correct: authoritative at paint time (see the module docs).
        self.hovered = ctx.is_hovered();
        let theme = Theme::from_paint_ctx(ctx);
        let ink = resolve_link_ink(self.hovered, theme);
        self.label.paint(ctx.origin(), ink, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, ctx.size()) {
                    return EventResult::Ignored;
                }
                self.captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured {
                    return EventResult::Handled;
                }
                let over = inside(p.position, ctx.size());
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(CursorIcon::Pointer);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                if inside(p.position, ctx.size())
                    && let Some(cb) = self.on_click.as_mut()
                {
                    (cb)(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Link, |node| {
            node.set_label(self.label.content());
            if self.on_click.is_some() {
                node.add_action(Action::Click);
            }
        });
    }
}

/// `BreadcrumbPage`: static current-page text, `font-normal text-foreground`
/// — no link semantics.
pub fn breadcrumb_page(label: impl Into<String>) -> frust::TextView {
    text(label)
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium)
        .themed_role(ThemeTextColor::OnSurface)
}

// ---- BreadcrumbSeparator / BreadcrumbEllipsis --------------------------

/// A declarative `BreadcrumbSeparator`. See the [module docs](self).
pub struct BreadcrumbSeparatorView<State: 'static> {
    custom: Option<AnyView<State>>,
}

/// The default chevron separator; call [`BreadcrumbSeparatorView::custom`]
/// to swap it for a caller-supplied icon.
pub fn breadcrumb_separator<State: 'static>() -> BreadcrumbSeparatorView<State> {
    BreadcrumbSeparatorView { custom: None }
}

impl<State: 'static> BreadcrumbSeparatorView<State> {
    /// Replace the default chevron with `child`.
    pub fn custom<V: View<State>>(mut self, child: V) -> Self {
        self.custom = Some(any(child));
        self
    }
}

/// The retained widget for a [`BreadcrumbSeparatorView`]: either a custom
/// child pod, or the default chevron (painted directly, no child).
pub struct BreadcrumbSeparatorWidget {
    custom: Option<ChildPod>,
}

impl<State: 'static> View<State> for BreadcrumbSeparatorView<State> {
    type Element = BreadcrumbSeparatorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BreadcrumbSeparatorWidget {
        BreadcrumbSeparatorWidget {
            custom: self
                .custom
                .as_ref()
                .map(|v| frust::authoring::build_child(v, ctx)),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BreadcrumbSeparatorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        match (
            prev.custom.as_ref(),
            self.custom.as_ref(),
            element.custom.as_mut(),
        ) {
            (Some(p), Some(n), Some(pod)) => frust::authoring::rebuild_child(p, n, pod, ctx),
            (None, None, _) => ChangeFlags::NONE,
            _ => {
                if let (Some(p), Some(pod)) = (prev.custom.as_ref(), element.custom.as_mut()) {
                    frust::authoring::teardown_child(p, pod, ctx);
                }
                element.custom = self
                    .custom
                    .as_ref()
                    .map(|v| frust::authoring::build_child(v, ctx));
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }

    fn teardown(&self, element: &mut BreadcrumbSeparatorWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(v), Some(pod)) = (self.custom.as_ref(), element.custom.as_mut()) {
            frust::authoring::teardown_child(v, pod, ctx);
        }
    }
}

fn resolve_separator_ink(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_MUTED, |t| t.scheme().on_surface_variant)
}

impl Widget for BreadcrumbSeparatorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if let Some(pod) = self.custom.as_mut() {
            let s = pod.layout_child(ctx, &BoxConstraints::loose(bc.max()));
            pod.set_origin(Point::ZERO);
            bc.constrain(s)
        } else {
            bc.constrain(Size::new(SEPARATOR_SIZE, SEPARATOR_SIZE))
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(pod) = self.custom.as_mut() {
            pod.paint_child(ctx, scene);
            return;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let ink = resolve_separator_ink(theme);
        let s = ctx.size();
        let origin = ctx.origin();
        // A `>` chevron, inset a quarter of the box on each side.
        let inset = s.width * 0.28;
        let top = Point::new(inset, inset);
        let mid = Point::new(s.width - inset, s.height / 2.0);
        let bottom = Point::new(inset, s.height - inset);
        scene.stroke_path(
            origin,
            &Line::new(top, mid).to_path(0.1),
            1.5,
            &Brush::Solid(ink),
        );
        scene.stroke_path(
            origin,
            &Line::new(mid, bottom).to_path(0.1),
            1.5,
            &Brush::Solid(ink),
        );
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match self.custom.as_mut() {
            Some(pod) => frust::authoring::route_event_single(pod, ctx, event),
            None => EventResult::Ignored,
        }
    }

    // `role="presentation" aria-hidden="true"`: decorative by default, so
    // the default no-op `semantics` (nothing contributed) is correct even
    // when a caller swaps in a custom icon — the separator stays presentational
    // either way, matching the source's fixed `aria-hidden` on the wrapper.
}

/// `BreadcrumbEllipsis`: a decorative `size-9` three-dot glyph.
pub struct BreadcrumbEllipsisView;

/// Create the decorative ellipsis marker.
pub fn breadcrumb_ellipsis() -> BreadcrumbEllipsisView {
    BreadcrumbEllipsisView
}

impl<State: 'static> View<State> for BreadcrumbEllipsisView {
    type Element = BreadcrumbEllipsisWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BreadcrumbEllipsisWidget {
        BreadcrumbEllipsisWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut BreadcrumbEllipsisWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// The retained widget for a [`BreadcrumbEllipsisView`].
pub struct BreadcrumbEllipsisWidget;

impl Widget for BreadcrumbEllipsisWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(ELLIPSIS_SIZE, ELLIPSIS_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let ink = resolve_separator_ink(theme);
        let s = ctx.size();
        let origin = ctx.origin();
        let cy = s.height / 2.0;
        let dot_r = 1.5;
        for i in 0..3 {
            let cx = s.width / 2.0 + (i as f64 - 1.0) * (dot_r * 2.0 + 3.0);
            scene.fill_rounded_rect(
                Point::new(origin.x + cx - dot_r, origin.y + cy - dot_r),
                Size::new(dot_r * 2.0, dot_r * 2.0),
                dot_r,
                ink,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rounded_rects: Vec<(Point, Size, f64, Color)>,
        glyph_colors: Vec<Color>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rounded_rects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    #[test]
    fn separator_default_paints_a_chevron() {
        let view: BreadcrumbSeparatorView<()> = breadcrumb_separator();
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
        assert_eq!(size, Size::new(SEPARATOR_SIZE, SEPARATOR_SIZE));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.strokes, 2, "two strokes form the chevron");
    }

    #[test]
    fn ellipsis_is_36px_and_paints_three_dots() {
        let view = breadcrumb_ellipsis();
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
        assert_eq!(size, Size::new(ELLIPSIS_SIZE, ELLIPSIS_SIZE));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rounded_rects.len(), 3);
    }

    #[test]
    fn list_lays_out_parts_left_to_right_with_the_1_5_gap() {
        let view: BreadcrumbListView<()> = breadcrumb_list(vec![
            any(frust_widgets::test_support::leaf(20.0, 14.0)),
            any(frust_widgets::test_support::leaf(20.0, 14.0)),
        ]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 40.0)));
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 20.0 + LIST_GAP);
    }

    // ---- Link hover: claim / latch-and-repaint / self-correct -----------

    struct HoverHarness {
        root: frust_core::RenderRoot<(), BreadcrumbLinkView<()>>,
        state: (),
        tcx: TextContext,
    }

    const WINDOW: Size = Size::new(200.0, 40.0);

    impl HoverHarness {
        fn new() -> Self {
            let mut h = Self {
                root: frust_core::RenderRoot::new(),
                state: (),
                tcx: TextContext::new(),
            };
            let mut app = |_s: &mut ()| breadcrumb_link::<()>("Docs");
            h.root.rebuild(&mut app, &mut h.state);
            h.root.layout_with_text(WINDOW, &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(
            &mut self,
            phase: PointerPhase,
            x: f64,
            y: f64,
        ) -> frust::authoring::EventOutcome {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(frust::authoring::PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: frust::authoring::PointerButton::Primary,
                }),
            )
        }

        fn glyph_color(&mut self) -> Color {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            rec.glyph_colors[0]
        }
    }

    #[test]
    fn hover_latches_a_redraw_and_recolors_the_link_to_foreground() {
        let mut h = HoverHarness::new();
        let unhovered = h.glyph_color();
        assert_eq!(unhovered, FALLBACK_MUTED);

        let gain = h.dispatch(PointerPhase::Move, 5.0, 10.0);
        assert!(gain.needs_redraw, "entering the link repaints (the latch)");
        assert_eq!(h.glyph_color(), FALLBACK_FOREGROUND);

        // Moving within the same link re-claims but changes nothing.
        let settled = h.dispatch(PointerPhase::Move, 6.0, 10.0);
        assert!(!settled.needs_redraw, "an unchanged flag asks for nothing");
    }

    #[test]
    fn up_ends_the_hover_link() {
        let mut h = HoverHarness::new();
        h.dispatch(PointerPhase::Move, 5.0, 10.0);
        assert_eq!(h.glyph_color(), FALLBACK_FOREGROUND);
        h.dispatch(PointerPhase::Down, 5.0, 10.0);
        let up = h.dispatch(PointerPhase::Up, 5.0, 10.0);
        assert!(up.needs_redraw);
        assert_eq!(h.glyph_color(), FALLBACK_MUTED, "Up ends the hover link");
    }

    #[test]
    fn on_click_fires_on_up_inside() {
        let view: BreadcrumbLinkView<u32> = breadcrumb_link("Docs").on_click(|s: &mut u32| *s += 1);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 40.0)));
        let mut state = 0u32;
        let mut ectx = EventCtx::new(&mut state, Point::ZERO, size);
        w.event(
            &mut ectx,
            &InputEvent::Pointer(frust::authoring::PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(5.0, 5.0),
                button: frust::authoring::PointerButton::Primary,
            }),
        );
        let mut ectx = EventCtx::new(&mut state, Point::ZERO, size);
        w.event(
            &mut ectx,
            &InputEvent::Pointer(frust::authoring::PointerEvent {
                phase: PointerPhase::Up,
                position: Point::new(5.0, 5.0),
                button: frust::authoring::PointerButton::Primary,
            }),
        );
        assert_eq!(state, 1);
    }

    // ---- Typeface: the link and page labels follow the live theme --------

    /// A link (a paint-recolored run shaped here), a separator (a path, no
    /// glyphs) and the current page (a `text` child).
    #[cfg(feature = "bundled-fonts")]
    fn trail(_: &mut ()) -> BreadcrumbListView<()> {
        breadcrumb(vec![
            any(breadcrumb_link::<()>("Home")),
            any(breadcrumb_separator::<()>()),
            any(breadcrumb_page("Settings")),
        ])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_link_and_page_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a breadcrumb's link and page",
            trail,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_link_and_page_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a breadcrumb's link and page",
            trail,
        );
    }
}
