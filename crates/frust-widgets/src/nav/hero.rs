//! Shared-element ("hero") transition wrapper: a transparent single-child
//! container that lets a [`navigator`](super::navigator) morph a tagged
//! element between two pages during a page transition.
//!
//! # How it works
//!
//! [`hero(tag, child)`](hero) wraps `child` and, on every paint, reports its
//! own absolute bounds under `tag` to whatever tagged-rect reporter the
//! navigator installed over the page
//! ([`PaintCtx::report_hero`](frust_core::PaintCtx::report_hero) — a no-op
//! returning [`HeroDirective::Normal`] the rest of the time, so a hero outside
//! a transition is a zero-cost transparent wrapper). When a transition is in
//! flight and the **same** tag is present on both the outgoing and incoming
//! page, the navigator hands one endpoint a [`HeroDirective::Morph`] (repaint
//! the child under a rect→rect transform onto the interpolated morph rect) and
//! the other a [`HeroDirective::Suppress`] (skip painting, so the element does
//! not also show at its rest position). The morph is painted by the hero on the
//! page drawn last (on top), keeping the overlay above both page layers whether
//! the transition is a push or a pop — see [`super::navigator`]'s
//! `paint_transition`.
//!
//! # Contract
//!
//! A hero is a *transparent* layout wrapper: it takes its child's size and
//! places it at the origin, forwards events and semantics to the child
//! unchanged, and never mutates app state. Heroes activate on any push/pop
//! transition where both pages carry the tag — no `Route`/`Router` change is
//! required.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, HeroDirective,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Rect, Size};

use super::transition::rect_to_rect;

/// A declarative shared-element wrapper. See the [module docs](self).
pub struct HeroView<State: 'static> {
    tag: String,
    child: AnyView<State>,
}

/// Tag `child` as a shared element under `tag`, so a [`navigator`](super::navigator)
/// morphs it between two pages that both carry the same tag during a page
/// transition. Outside a transition this is a transparent wrapper.
pub fn hero<State: 'static, V: View<State>>(tag: impl Into<String>, child: V) -> HeroView<State> {
    HeroView {
        tag: tag.into(),
        child: any(child),
    }
}

/// The retained widget for a [`HeroView`].
pub struct HeroWidget {
    tag: String,
    child: ChildPod,
}

impl<State: 'static> View<State> for HeroView<State> {
    type Element = HeroWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> HeroWidget {
        HeroWidget {
            tag: self.tag.clone(),
            child: crate::authoring::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut HeroWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.tag != self.tag {
            element.tag = self.tag.clone();
            // A tag change only affects which heroes match during a transition —
            // no relayout, just a repaint.
            flags |= ChangeFlags::PAINT;
        }
        flags |= crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut HeroWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for HeroWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Transparent wrapper: the hero is exactly its child, placed at the
        // origin — so the bounds it reports on paint are the child's own.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let bounds = Rect::from_origin_size(ctx.origin(), ctx.size());
        match ctx.report_hero(&self.tag, bounds) {
            // Common case (no flight, or a tag on one side only): paint normally.
            HeroDirective::Normal => self.child.paint_child(ctx, scene),
            // This endpoint is being morphed by its counterpart on the other
            // page — do not also paint it at its rest position.
            HeroDirective::Suppress => {}
            // Paint the morph overlay: repaint the retained child subtree under
            // a transform mapping our own paint bounds onto the interpolated
            // destination rect (position + scale).
            HeroDirective::Morph { dest } => {
                scene.push_transform(rect_to_rect(bounds, dest));
                self.child.paint_child(ctx, scene);
                scene.pop_transform();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent wrapper: forward to the single child.
        self.child.semantics_child(ctx);
    }
}
