//! `fill_box` / `filled_box` — the arbitrary-color rounded-rect escape-hatch
//! primitives no facade widget paints (promoted here from `screens::home`
//! so the feed restyle can share them).
//!
//! Two small hand-rolled [`View`]/[`Widget`] pairs built directly against
//! `frust-core` — the facade's documented "low-level escape hatch" pattern
//! (`docs/ARCHITECTURE.md`), the same precedent `ui::swipeable`/`ui::toast`
//! already use:
//!
//! - [`FillBox`] is a fixed-size filled rounded rect (a circle is
//!   `radius == size / 2`) — the avatar / status-dot / badge / composer-tile
//!   primitive. It is a **leaf**: it paints its own `size`, ignoring its child
//!   (there is none), so a monogram is layered over it with the
//!   `SizedBox(n,n).child(Align(CENTER, …))` idiom in a `Stack` (see
//!   `screens::home::channel_circle`).
//! - [`FilledBox`] is the **single-child** counterpart: a rounded fill painted
//!   *behind* a child it sizes itself to — i.e. `filled_card` WITHOUT the
//!   material card's hardcoded 16px content inset (`material/card.rs`). This is
//!   what lets a compact attachment tile keep a boxed look
//!   (`filled_box(Padding(10‥12, …), color, 8)`) without the card inset that
//!   ballooned the whole feed.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};
use peniko::Color;

// ---------------------------------------------------------------------------
// FillBox — fixed-size filled rounded rect (leaf)
// ---------------------------------------------------------------------------

/// A filled rounded rect of an arbitrary color — the avatar/status-dot/badge/
/// composer-tile primitive no facade widget paints. A circle is a [`FillBox`]
/// with `radius == size / 2`.
pub struct FillBox {
    size: Size,
    color: Color,
    radius: f64,
}

/// The retained widget for a [`FillBox`].
pub struct FillBoxWidget {
    size: Size,
    color: Color,
    radius: f64,
}

/// Construct a [`FillBox`].
pub fn fill_box(size: Size, color: Color, radius: f64) -> FillBox {
    FillBox {
        size,
        color,
        radius,
    }
}

impl<State: 'static> View<State> for FillBox {
    type Element = FillBoxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FillBoxWidget {
        FillBoxWidget {
            size: self.size,
            color: self.color,
            radius: self.radius,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut FillBoxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.size = self.size;
        element.color = self.color;
        element.radius = self.radius;
        ChangeFlags::PAINT
    }
}

impl Widget for FillBoxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), self.radius, self.color);
    }
}

// ---------------------------------------------------------------------------
// FilledBox — single-child container painting a rounded fill behind its child
// ---------------------------------------------------------------------------

/// A rounded, filled single-child container — the escape-hatch equivalent of
/// `filled_card` WITHOUT the card's hardcoded 16px inset (`material/card.rs`),
/// so a compact boxed look (attachment tiles) is expressible without inflating
/// the row. Wrap the padded content: `filled_box(Padding(10‥12, child), c, 8)`.
pub struct FilledBox<State: 'static> {
    child: AnyView<State>,
    color: Color,
    radius: f64,
}

/// Wrap `child` in a rounded [`FilledBox`] filled with `color`.
pub fn filled_box<State: 'static, V: View<State>>(
    child: V,
    color: Color,
    radius: f64,
) -> FilledBox<State> {
    FilledBox {
        child: any(child),
        color,
        radius,
    }
}

/// The retained widget for a [`FilledBox`].
pub struct FilledBoxWidget {
    child: ChildPod,
    color: Color,
    radius: f64,
}

/// Build a [`ChildPod`] wrapping an [`AnyView`]'s element (mirrors
/// `frust-widgets::build_child`, re-derived because that helper is
/// crate-private — same shape `ui::swipeable` re-derives).
fn build_child<State: 'static>(view: &AnyView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

/// Reconcile the child through its `ChildPod` (mirrors
/// `frust-widgets::rebuild_child`).
fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("filled_box child element is a boxed AnyView widget");
    next.rebuild(prev, element, ctx)
}

/// Tear the child down (mirrors `frust-widgets::teardown_child`).
fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

impl<State: 'static> View<State> for FilledBox<State> {
    type Element = FilledBoxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FilledBoxWidget {
        FilledBoxWidget {
            child: build_child(&self.child, ctx),
            color: self.color,
            radius: self.radius,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FilledBoxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.color = self.color;
        element.radius = self.radius;
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx) | ChangeFlags::PAINT
    }

    fn teardown(&self, element: &mut FilledBoxWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for FilledBoxWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), self.radius, self.color);
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }
}
