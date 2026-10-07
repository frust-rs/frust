//! Ports shadcn/ui's **ButtonGroup** from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/button-group.tsx`.
//!
//! `ButtonGroup` itself is pure layout: `role="group"`, `flex` (row or
//! column per [`ButtonGroupOrientation`]), `items-stretch`, no gap — its
//! members abut flush. The source's inner-radius/shared-border collapse
//! (`[&>*:not(:first-child)]:rounded-l-none`, `border-l-0`, …) is a
//! child-selector cascade with no frust counterpart (there is no
//! parent-driven restyle of an arbitrary child's own paint), so it is not
//! reproduced here — a member paints its own radius/border exactly as it
//! would standalone. See the [module docs](self)'s *Scope* note for what
//! this means for callers; a flush-border restyle of members is a known
//! deferred follow-on.
//!
//! [`ButtonGroupText`]/[`ButtonGroupSeparator`] are not ported as distinct
//! types: `ButtonGroupText` is an ordinary bordered/shadowed pill a caller
//! composes from existing primitives ([`crate::style`] carries the same
//! `rounded-md`/`shadow-xs` vocabulary), and `ButtonGroupSeparator` is
//! `separator.rs`'s own [`crate::SeparatorView`] with `bg-input` — nothing
//! group-specific to port.
//!
//! # Scope
//!
//! The container itself claims no hover/press/focus of its own — it is
//! `flex`, not a control — and forwards every event to its members
//! unmodified, exactly like [`crate::components::breadcrumb`]'s list.

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, View, ViewSeq, Widget,
};

/// `buttonGroupVariants`' `orientation` axis. `Horizontal` = shadcn's
/// `default`/`horizontal`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonGroupOrientation {
    /// A row: members flow left to right.
    #[default]
    Horizontal,
    /// A column: members flow top to bottom.
    Vertical,
}

/// A declarative shadcn button group. See the [module docs](self).
pub struct ButtonGroupView<State: 'static> {
    children: Vec<AnyView<State>>,
    orientation: ButtonGroupOrientation,
}

/// Group `children` (buttons, inputs, a select — any widget) flush together,
/// horizontally by default.
///
/// The list is any [`ViewSeq`] — a tuple of mixed view types (`(a, b, c)`),
/// a `Vec`/array of one type, an `Option`, or `views(iter)` — erased once here,
/// so no element needs `any(..)`.
pub fn button_group<State: 'static, M>(children: impl ViewSeq<State, M>) -> ButtonGroupView<State> {
    let mut erased = Vec::new();
    children.extend_views(&mut erased);
    let children = erased;
    ButtonGroupView {
        children,
        orientation: ButtonGroupOrientation::default(),
    }
}

impl<State: 'static> ButtonGroupView<State> {
    /// Set the [`ButtonGroupOrientation`].
    pub fn orientation(mut self, orientation: ButtonGroupOrientation) -> Self {
        self.orientation = orientation;
        self
    }
}

/// The retained widget for a [`ButtonGroupView`].
pub struct ButtonGroupWidget {
    children: Vec<ChildPod>,
    orientation: ButtonGroupOrientation,
}

impl<State: 'static> View<State> for ButtonGroupView<State> {
    type Element = ButtonGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ButtonGroupWidget {
        ButtonGroupWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
            orientation: self.orientation,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        );
        if element.orientation != self.orientation {
            element.orientation = self.orientation;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ButtonGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ButtonGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let max = bc.max();
        // `items-stretch`: every member gets the group's own cross-axis
        // extent (when finite) so members of differing natural cross size
        // still abut flush; an unbounded cross axis loosens instead.
        let cross_bound = match self.orientation {
            ButtonGroupOrientation::Horizontal if max.height.is_finite() => Some(max.height),
            ButtonGroupOrientation::Vertical if max.width.is_finite() => Some(max.width),
            _ => None,
        };

        let mut main = 0.0_f64;
        let mut cross = 0.0_f64;
        let mut sizes = Vec::with_capacity(self.children.len());
        for pod in self.children.iter_mut() {
            let child_bc = match self.orientation {
                ButtonGroupOrientation::Horizontal => BoxConstraints::new(
                    Size::new(0.0, cross_bound.unwrap_or(0.0)),
                    Size::new(f64::INFINITY, cross_bound.unwrap_or(f64::INFINITY)),
                ),
                ButtonGroupOrientation::Vertical => BoxConstraints::new(
                    Size::new(cross_bound.unwrap_or(0.0), 0.0),
                    Size::new(cross_bound.unwrap_or(f64::INFINITY), f64::INFINITY),
                ),
            };
            let s = pod.layout_child(ctx, &child_bc);
            sizes.push(s);
            match self.orientation {
                ButtonGroupOrientation::Horizontal => {
                    main += s.width;
                    cross = cross.max(s.height);
                }
                ButtonGroupOrientation::Vertical => {
                    main += s.height;
                    cross = cross.max(s.width);
                }
            }
        }

        let mut offset = 0.0_f64;
        for (pod, s) in self.children.iter_mut().zip(sizes.iter()) {
            match self.orientation {
                ButtonGroupOrientation::Horizontal => {
                    pod.set_origin(Point::new(offset, 0.0));
                    offset += s.width;
                }
                ButtonGroupOrientation::Vertical => {
                    pod.set_origin(Point::new(0.0, offset));
                    offset += s.height;
                }
            }
        }

        let size = match self.orientation {
            ButtonGroupOrientation::Horizontal => Size::new(main, cross),
            ButtonGroupOrientation::Vertical => Size::new(cross, main),
        };
        bc.constrain(size)
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
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_widgets::test_support::leaf;

    fn build<S: 'static>(view: &ButtonGroupView<S>) -> ButtonGroupWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ButtonGroupWidget, max: Size) -> Size {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(max))
    }

    #[test]
    fn horizontal_members_abut_flush_left_to_right() {
        // An unbounded cross axis (height): `items-stretch` has nothing to
        // stretch to, so each member keeps its own natural height.
        let view: ButtonGroupView<()> = button_group(vec![leaf(40.0, 20.0), leaf(60.0, 20.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(500.0, f64::INFINITY));
        assert_eq!(size, Size::new(100.0, 20.0));
        assert_eq!(w.children[0].origin(), Point::ZERO);
        assert_eq!(w.children[1].origin(), Point::new(40.0, 0.0));
    }

    #[test]
    fn vertical_members_stack_flush_top_to_bottom() {
        let view: ButtonGroupView<()> = button_group(vec![leaf(40.0, 20.0), leaf(40.0, 30.0)])
            .orientation(ButtonGroupOrientation::Vertical);
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(f64::INFINITY, 500.0));
        assert_eq!(size, Size::new(40.0, 50.0));
        assert_eq!(w.children[0].origin(), Point::ZERO);
        assert_eq!(w.children[1].origin(), Point::new(0.0, 20.0));
    }

    #[test]
    fn a_finite_cross_axis_stretches_every_member_to_it() {
        let view: ButtonGroupView<()> = button_group(vec![leaf(40.0, 20.0), leaf(60.0, 10.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(500.0, 500.0));
        assert_eq!(
            size.height, 500.0,
            "items-stretch fills the finite cross axis"
        );
    }

    #[test]
    fn no_gap_between_members() {
        let view: ButtonGroupView<()> =
            button_group(vec![leaf(10.0, 10.0), leaf(10.0, 10.0), leaf(10.0, 10.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(500.0, 500.0));
        assert_eq!(size.width, 30.0, "three 10px members, zero gap");
    }

    #[test]
    fn event_routing_reaches_a_member() {
        use frust::authoring::{PointerButton, PointerEvent, PointerPhase};
        let view: ButtonGroupView<Vec<u32>> =
            button_group(vec![frust_widgets::test_support::probe(0)]);
        let mut w = build(&view);
        layout(&mut w, Size::new(100.0, 100.0));
        let mut log: Vec<u32> = Vec::new();
        let mut ectx = EventCtx::new(&mut log, Point::ZERO, Size::new(100.0, 100.0));
        let result = w.event(
            &mut ectx,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(5.0, 5.0),
                button: PointerButton::Primary,
            }),
        );
        assert_eq!(result, EventResult::Handled);
        assert_eq!(log, vec![0]);
    }
}
