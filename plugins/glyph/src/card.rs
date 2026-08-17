//! [`glyph_card`]/[`GlyphCardView`]: the Glyph content card — a `surface`
//! container with a bright hairline border and the `radius-lg` corner, holding
//! up to three optional child-view **slots** stacked vertically: `title`,
//! `desc`, and `footer` (the Glyph design system's `.card` with
//! `.card-title`/`.card-desc`/`.card-footer`).
//!
//! Each slot is an arbitrary child [`frust::View`] (not a fixed text run),
//! so an app composes any content into a card while the card owns only the
//! chrome (background/border/radius/padding) and the vertical stacking. The
//! card itself is non-interactive; events route straight through to whichever
//! slot they hit (a footer button, say), and every present slot's semantics is
//! forwarded (`semantics_child`) under one `GenericContainer` node — the
//! silent-drop rule.
//!
//! # Token resolution
//!
//! - **background** = `surface_container` (Glyph `bg-surface`, "cards, inputs").
//! - **border** = `outline` (Glyph's bright hairline `border-bright`).
//! - **radius** = `shape.large` (Glyph `--radius-lg` = 16px).
//!
//! Unthemed, each falls back to the literal Glyph **dark** constant.

use frust::authoring::Role;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use frust::{ShapeScale, Theme};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

/// Corner-rounding tolerance for the border stroke.
const PATH_TOLERANCE: f64 = 0.1;
/// Content padding on all four edges, in logical px (`.card{padding:16px}`).
const CARD_PADDING: f64 = 16.0;
/// Vertical gap between adjacent present slots, in logical px.
const CARD_SLOT_GAP: f64 = 10.0;
/// Border width, in logical px.
const CARD_BORDER_WIDTH: f64 = 1.0;
/// Unthemed corner-radius fallback — Glyph `--radius-lg` (16px).
const CARD_RADIUS_FALLBACK: f64 = 16.0;

/// Unthemed background — Glyph dark `bg-surface` (`#161a23`).
const CARD_BG: Color = Color::from_rgb8(0x16, 0x1a, 0x23);
/// Unthemed border — Glyph dark `border-bright`/`outline` (`#3e3f44`).
const CARD_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);

/// A declarative Glyph content card. See the [module docs](self).
pub struct GlyphCardView<State: 'static> {
    title: Option<AnyView<State>>,
    desc: Option<AnyView<State>>,
    footer: Option<AnyView<State>>,
}

/// Create an empty Glyph card. Chain [`GlyphCardView::title`]/
/// [`GlyphCardView::desc`]/[`GlyphCardView::footer`] to fill its slots.
pub fn glyph_card<State: 'static>() -> GlyphCardView<State> {
    GlyphCardView {
        title: None,
        desc: None,
        footer: None,
    }
}

impl<State: 'static> GlyphCardView<State> {
    /// Set the title slot (the topmost row).
    pub fn title<V: View<State>>(mut self, view: V) -> Self {
        self.title = Some(any(view));
        self
    }

    /// Set the description slot (the middle row).
    pub fn desc<V: View<State>>(mut self, view: V) -> Self {
        self.desc = Some(any(view));
        self
    }

    /// Set the footer slot (the bottom row).
    pub fn footer<V: View<State>>(mut self, view: V) -> Self {
        self.footer = Some(any(view));
        self
    }
}

/// A single reconciled slot: its retained pod plus whether it is present.
#[derive(Default)]
struct Slot {
    pod: Option<ChildPod>,
}

impl frust::authoring::VisitPods for Slot {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        frust::authoring::VisitPods::visit_pods(&self.pod, visitor);
    }
}

impl Slot {
    /// Reconcile this slot against the `(prev, next)` optional child views.
    fn rebuild<State: 'static>(
        &mut self,
        prev: &Option<AnyView<State>>,
        next: &Option<AnyView<State>>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        match (prev, next, self.pod.as_mut()) {
            (Some(p), Some(n), Some(pod)) => frust::authoring::rebuild_child(p, n, pod, ctx),
            (None, Some(n), _) => {
                self.pod = Some(frust::authoring::build_child(n, ctx));
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
            (Some(p), None, Some(_)) => {
                if let Some(mut pod) = self.pod.take() {
                    frust::authoring::teardown_child(p, &mut pod, ctx);
                }
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
            _ => ChangeFlags::NONE,
        }
    }
}

/// The retained widget for a [`GlyphCardView`].
pub struct GlyphCardWidget<State: 'static> {
    title: Slot,
    desc: Slot,
    footer: Slot,
    _state: std::marker::PhantomData<State>,
}

impl<State: 'static> GlyphCardWidget<State> {
    /// Iterate every present slot pod in paint order (title, desc, footer).
    fn pods_mut(&mut self) -> impl Iterator<Item = &mut ChildPod> {
        [
            self.title.pod.as_mut(),
            self.desc.pod.as_mut(),
            self.footer.pod.as_mut(),
        ]
        .into_iter()
        .flatten()
    }
}

impl<State: 'static> View<State> for GlyphCardView<State> {
    type Element = GlyphCardWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlyphCardWidget<State> {
        let mk = |v: &Option<AnyView<State>>, ctx: &mut BuildCtx<'_>| Slot {
            pod: v
                .as_ref()
                .map(|view| frust::authoring::build_child(view, ctx)),
        };
        GlyphCardWidget {
            title: mk(&self.title, ctx),
            desc: mk(&self.desc, ctx),
            footer: mk(&self.footer, ctx),
            _state: std::marker::PhantomData,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphCardWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        flags |= element.title.rebuild(&prev.title, &self.title, ctx);
        flags |= element.desc.rebuild(&prev.desc, &self.desc, ctx);
        flags |= element.footer.rebuild(&prev.footer, &self.footer, ctx);
        flags
    }

    fn teardown(&self, element: &mut GlyphCardWidget<State>, ctx: &mut BuildCtx<'_>) {
        for (view, slot) in [
            (&self.title, &mut element.title),
            (&self.desc, &mut element.desc),
            (&self.footer, &mut element.footer),
        ] {
            if let (Some(v), Some(pod)) = (view.as_ref(), slot.pod.as_mut()) {
                frust::authoring::teardown_child(v, pod, ctx);
            }
        }
    }
}

fn resolve_chrome(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            (s.surface_container, s.outline)
        }
        None => (CARD_BG, CARD_BORDER),
    }
}

fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.large, size.width, size.height),
        None => CARD_RADIUS_FALLBACK,
    }
}

impl<State: 'static> Widget for GlyphCardWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max_w = (bc.max().width - CARD_PADDING * 2.0).max(0.0);
        let inner_bc = BoxConstraints::new(Size::ZERO, Size::new(inner_max_w, f64::INFINITY));

        let mut y = CARD_PADDING;
        let mut max_child_w = 0.0f64;
        let mut first = true;
        for pod in self.pods_mut() {
            if !first {
                y += CARD_SLOT_GAP;
            }
            first = false;
            let child_size = pod.layout_child(ctx, &inner_bc);
            pod.set_origin(Point::new(CARD_PADDING, y));
            y += child_size.height;
            max_child_w = max_child_w.max(child_size.width);
        }

        let width = max_child_w + CARD_PADDING * 2.0;
        let height = if first {
            // No slots: a minimal padded box.
            CARD_PADDING * 2.0
        } else {
            y + CARD_PADDING
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, border) = resolve_chrome(theme);
        let radius = resolve_radius(theme, ctx.size());
        let o = ctx.origin();
        let size = ctx.size();

        scene.fill_rounded_rect(o, size, radius, bg);
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(o, &path, CARD_BORDER_WIDTH, &Brush::Solid(border));

        for pod in self.pods_mut() {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Broadcast-first, then active-pod (a captured gesture must not
        // re-hit-test), then focus-routed, then reverse-paint-order hit test —
        // the same contract as `frust::authoring::route_event`, over this
        // widget's named optional slots.
        if event.is_broadcast() {
            for pod in self.pods_mut() {
                // Never consumed: every slot gets it, results discarded.
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if let Some(pod) = self.pods_mut().find(|p| p.is_active()) {
            return frust::authoring::route_event_single(pod, ctx, event);
        }
        if event.is_focus_routed() {
            if let Some(pod) = self.pods_mut().find(|p| p.is_focused()) {
                return pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        let position = event.position();
        let mut pods: Vec<&mut ChildPod> = self.pods_mut().collect();
        for pod in pods.iter_mut().rev() {
            if pod.contains(position) && pod.event_child(ctx, event) == EventResult::Handled {
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::GenericContainer,
            |_| {},
            |ctx| {
                for slot in [&self.title, &self.desc, &self.footer] {
                    if let Some(pod) = slot.pod.as_ref() {
                        pod.semantics_child(ctx);
                    }
                }
            },
        );
    }

    frust::authoring::visit_children!(title, desc, footer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;
    use frust::authoring::text::TextContext;
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
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
        fn draw_glyph_run(&mut self, _run: frust::authoring::scene::GlyphRun) {}
    }

    fn build<S: 'static>(view: &GlyphCardView<S>) -> GlyphCardWidget<S> {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_and_paint<S: 'static>(
        widget: &mut GlyphCardWidget<S>,
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
    fn unthemed_chrome_uses_fallback_constants() {
        let view: GlyphCardView<()> = glyph_card().title(text("Hello"));
        let mut w = build(&view);
        let rec = layout_and_paint(&mut w, None);
        assert_eq!(rec.rrects[0].3, CARD_BG);
        assert_eq!(rec.rrects[0].2, CARD_RADIUS_FALLBACK);
        assert_eq!(rec.strokes[0], CARD_BORDER);
    }

    #[test]
    fn glyph_dark_and_light_chrome_differ() {
        let dark = crate::baseline();
        let light = dark.clone().with_brightness(Brightness::Light);
        let view: GlyphCardView<()> = glyph_card().title(text("Hi"));
        let mut wd = build(&view);
        let mut wl = build(&view);
        let rec_d = layout_and_paint(&mut wd, Some(&dark));
        let rec_l = layout_and_paint(&mut wl, Some(&light));
        assert_eq!(rec_d.rrects[0].3, dark.scheme().surface_container);
        assert_eq!(rec_d.strokes[0], dark.scheme().outline);
        assert_eq!(rec_l.rrects[0].3, light.scheme().surface_container);
        assert_ne!(rec_d.rrects[0].3, rec_l.rrects[0].3);
    }

    #[test]
    fn slots_stack_vertically_with_gaps() {
        let view: GlyphCardView<()> = glyph_card()
            .title(frust::SizedBox::<()>(Some(80.0), Some(20.0)))
            .desc(frust::SizedBox::<()>(Some(60.0), Some(30.0)))
            .footer(frust::SizedBox::<()>(Some(100.0), Some(10.0)));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        // Widest child (100) + padding both sides.
        assert_eq!(size.width, 100.0 + CARD_PADDING * 2.0);
        // 3 rows (20+30+10) + 2 gaps + top/bottom padding.
        let expected_h = 20.0 + 30.0 + 10.0 + CARD_SLOT_GAP * 2.0 + CARD_PADDING * 2.0;
        assert_eq!(size.height, expected_h);
        assert_eq!(w.title.pod.as_ref().unwrap().origin().y, CARD_PADDING);
        assert_eq!(
            w.desc.pod.as_ref().unwrap().origin().y,
            CARD_PADDING + 20.0 + CARD_SLOT_GAP
        );
    }

    #[test]
    fn footer_slot_receives_press_events() {
        #[derive(Default)]
        struct Counter {
            presses: u32,
        }
        let view: GlyphCardView<Counter> = glyph_card()
            .title(frust::SizedBox::<Counter>(Some(80.0), Some(20.0)))
            .footer(frust::button::<Counter, _>("go", |s: &mut Counter| {
                s.presses += 1
            }));
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));

        let footer_origin = w.footer.pod.as_ref().unwrap().origin();
        let mut state = Counter::default();
        let ev = |phase, x: f64, y: f64| {
            InputEvent::Pointer(frust_core::PointerEvent {
                phase,
                position: Point::new(x, y),
                button: frust_core::PointerButton::Primary,
            })
        };
        let dispatch = |w: &mut GlyphCardWidget<Counter>, state: &mut Counter, e: &InputEvent| {
            let sa: &mut dyn Any = state;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(400.0, 400.0));
            w.event(&mut ctx, e)
        };
        dispatch(
            &mut w,
            &mut state,
            &ev(
                frust_core::PointerPhase::Down,
                footer_origin.x + 4.0,
                footer_origin.y + 4.0,
            ),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(
                frust_core::PointerPhase::Up,
                footer_origin.x + 4.0,
                footer_origin.y + 4.0,
            ),
        );
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn removing_a_slot_on_rebuild_tears_it_down() {
        let with_desc: GlyphCardView<()> = glyph_card().title(text("t")).desc(text("d"));
        let mut w = build(&with_desc);
        assert!(w.desc.pod.is_some());
        let without_desc: GlyphCardView<()> = glyph_card().title(text("t"));
        let mut counter = 0u64;
        <GlyphCardView<()> as View<()>>::rebuild(
            &without_desc,
            &with_desc,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert!(w.desc.pod.is_none());
        assert!(w.title.pod.is_some());
    }

    #[test]
    fn semantics_forwards_every_present_slot_under_a_container() {
        fn logic(_s: &mut ()) -> GlyphCardView<()> {
            glyph_card().title(text("Title")).desc(text("Desc"))
        }
        let mut root: frust_core::RenderRoot<(), GlyphCardView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::GenericContainer)
            .expect("card contributes a Role::GenericContainer node");
        assert_eq!(
            node.children().len(),
            2,
            "both present slots are semantics children"
        );
    }
}
