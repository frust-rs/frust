//! Hot-patch seam (opt-in `hotpatch` feature): every `Component::build` runs through subsecond's jump
//! table, so a `dx` patch that swaps the monomorphised call takes effect on the next rebuild.

use std::sync::Arc;

use subsecond::HotFn;

use crate::component::Component;
use crate::view::AnyView;

/// Call `component.build(state)` through the jump table and erase the result.
///
/// This is the single erasure point `ComponentView` uses under the feature (the non-feature path keeps
/// its inline `AnyView::new`). With no patch applied subsecond falls through to the original function.
/// `init` is deliberately not routed here: it must not re-run on a patch.
pub(crate) fn call_build<C: Component>(component: &C, state: &mut C::State) -> AnyView<C::State> {
    AnyView::new(HotFn::current(<C as Component>::build).call((component, state)))
}

/// Register `listener` to run right after a patch library is loaded and the jump table swapped.
///
/// It runs on the thread that applied the patch, not the UI thread: it must only signal (e.g. send
/// through an event-loop proxy).
pub fn set_patch_listener(listener: Arc<dyn Fn() + Send + Sync>) {
    subsecond::register_handler(listener);
}

#[cfg(all(test, feature = "hotpatch"))]
mod tests {
    use super::*;
    use crate::view::{BuildCtx, ChangeFlags, View};
    use crate::widget::{LayoutCtx, PaintCtx, PaintScene, Widget};
    use kurbo::Size;
    use std::any::Any;

    use crate::layout::BoxConstraints;

    struct Leaf;
    struct LeafWidget;
    impl Widget for LeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }
    impl View<u32> for Leaf {
        type Element = LeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
            LeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut LeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    struct Probe;
    impl Component for Probe {
        type State = u32;
        fn init(&self) -> u32 {
            0
        }
        fn build(&self, state: &mut u32) -> impl View<u32> {
            *state += 1;
            Leaf
        }
    }

    #[test]
    fn call_build_matches_direct_build_with_no_patch() {
        let c = Probe;
        let mut state = 0u32;
        let direct = AnyView::new(c.build(&mut state));
        let via_seam = call_build(&c, &mut state);
        // Both builds ran the real `build` exactly once each.
        assert_eq!(state, 2);
        let mut id = 0u64;
        let mut ctx = BuildCtx::new(&mut id);
        let a = direct.build(&mut ctx);
        let b = via_seam.build(&mut ctx);
        let (a, b): (&dyn Any, &dyn Any) = (&*a, &*b);
        assert_eq!(a.type_id(), b.type_id());
    }
}
