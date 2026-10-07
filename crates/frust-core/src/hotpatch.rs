//! Hot-patch seam (opt-in `hotpatch` feature): every `Component::build` runs through frust-hotpatch's
//! jump table, so a `dx` patch that swaps the monomorphised call takes effect on the next rebuild.
//!
//! The seam's boundary types are layout-fixed. [`build_erased`] erases the component's view inside
//! the hot function, so the return slot is `Result<AnyView<_>, LayoutMismatch>` whatever the view
//! type is; and it takes a [`SeamWitness`] written by the image that created the state, which the
//! (possibly patched) callee compares with its own layout before it touches that state.

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use frust_hotpatch::HotFn;
pub use frust_hotpatch::LayoutMismatch;
use kurbo::Size;

use crate::component::Component;
use crate::layout::BoxConstraints;
use crate::view::{AnyView, BuildCtx, ChangeFlags, View};
use crate::widget::{LayoutCtx, PaintCtx, PaintScene, Widget};

/// The layout a component's types have in one image: the size and alignment of `C::State` and of
/// `C` itself.
///
/// Written once by the image that runs `init` (so creates the state) and compared, before any
/// `C`-generic code touches that state, with [`SeamWitness::of`] as computed by the image doing the
/// touching. A difference means a patch changed the layout under a live value: the toucher reports
/// it ([`frust_hotpatch::report_layout_mismatch`]) and backs off. Four `u32`s in `repr(C)`, so the
/// witness itself has the same layout in every image.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SeamWitness {
    /// `size_of::<C::State>()`.
    pub size_state: u32,
    /// `align_of::<C::State>()`.
    pub align_state: u32,
    /// `size_of::<C>()`.
    pub size_c: u32,
    /// `align_of::<C>()`.
    pub align_c: u32,
}

impl SeamWitness {
    /// This image's own layout for `C` and `C::State`.
    pub fn of<C: Component>() -> Self {
        Self {
            size_state: layout_u32(size_of::<C::State>()),
            align_state: layout_u32(align_of::<C::State>()),
            size_c: layout_u32(size_of::<C>()),
            align_c: layout_u32(align_of::<C>()),
        }
    }
}

/// Saturating: a layout past `u32::MAX` bytes cannot be told apart from another such, which no real
/// component state reaches.
fn layout_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl fmt::Display for SeamWitness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "state size {} align {}, component size {} align {}",
            self.size_state, self.align_state, self.size_c, self.align_c
        )
    }
}

/// Compare `stored` (written by the state's creating image) with this image's own layout for `C`.
/// On a difference the mismatch is reported to frust-hotpatch and returned; the caller must then
/// leave the state, and everything built over it, alone.
pub(crate) fn verify_witness<C: Component>(stored: SeamWitness) -> Result<(), LayoutMismatch> {
    let own = SeamWitness::of::<C>();
    if stored == own {
        return Ok(());
    }
    let type_name = type_name::<C::State>();
    frust_hotpatch::report_layout_mismatch(type_name, stored, own);
    Err(LayoutMismatch {
        type_name: type_name.to_owned(),
        stored: stored.to_string(),
        own: own.to_string(),
    })
}

/// Run `c.build(s)` through the jump table and return its view erased.
///
/// The hot function checks `witness` against its own layout first and returns the mismatch (already
/// reported) without touching `s` when they differ. `witness` must come from storage the state's
/// creating image wrote: a nested component passes its widget's stored witness, a root driver (the
/// creator of the root state) its own `SeamWitness::of::<Root>()`. The erasure happens inside the
/// hot function, so the type crossing the return slot is the same in every image. With no patch
/// applied frust-hotpatch falls through to the original function. `init` is deliberately not
/// routed here: it must not re-run on a patch.
// erasure: keep hot-patch boundary type must be layout-fixed
pub fn build_erased<C: Component>(
    c: &C,
    s: &mut C::State,
    witness: SeamWitness,
) -> Result<AnyView<C::State>, LayoutMismatch> {
    HotFn::current(checked_build::<C>).call((c, s, witness))
}

/// The hot function behind [`build_erased`]: the jump table keys on this monomorphisation, so a
/// patch replaces the check and the erasure together with `build`.
fn checked_build<C: Component>(
    c: &C,
    s: &mut C::State,
    witness: SeamWitness,
) -> Result<AnyView<C::State>, LayoutMismatch> {
    verify_witness::<C>(witness)?;
    Ok(AnyView::new(c.build(s)))
}

/// A seam result as a view: the built view, or [`Inert`] in place of one the seam refused.
pub(crate) fn built_or_inert<S: 'static>(built: Result<AnyView<S>, LayoutMismatch>) -> AnyView<S> {
    built.unwrap_or_else(|_| AnyView::new(Inert))
}

/// Stands in for a subtree the seam refused to build: lays out at zero size and draws nothing.
pub(crate) struct Inert;

/// [`Inert`]'s element.
pub(crate) struct InertWidget;

impl Widget for InertWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

impl<S: 'static> View<S> for Inert {
    type Element = InertWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> InertWidget {
        InertWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut InertWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// The view a root driver hands its shell under the hot-patch feature: the root component's view
/// from [`build_erased`], or nothing when the seam reported a layout mismatch.
///
/// A mismatch keeps the tree already on screen: the rebuild does nothing (as a nested component's
/// skipped rebuild does) and the stale tree stays until the session restarts, which the reported
/// record requests. A first build that mismatches shows an empty root.
pub struct RootView<S: 'static> {
    view: Option<AnyView<S>>,
}

impl<S: 'static> RootView<S> {
    /// Wrap the result of the root component's [`build_erased`].
    pub fn new(built: Result<AnyView<S>, LayoutMismatch>) -> Self {
        Self { view: built.ok() }
    }
}

impl<S: 'static> View<S> for RootView<S> {
    type Element = Box<dyn Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Box<dyn Widget> {
        match &self.view {
            Some(view) => view.build(ctx),
            None => Box::new(InertWidget),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Box<dyn Widget>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        match (&self.view, &prev.view) {
            (Some(view), Some(prev)) => view.rebuild(prev, element, ctx),
            // Refused: leave the live tree exactly as it is.
            (None, _) => ChangeFlags::NONE,
            // The previous frame was refused, so no view is left to diff against or to tear the
            // live tree down through: replace it outright (its widgets' `Drop` still disposes
            // component owners). Only reachable if a patch is applied over an unreported
            // mismatch, which the devtools apply entry refuses.
            (Some(view), None) => {
                if ctx.has_focus() {
                    crate::event::mark_focus_orphaned();
                }
                *element = view.build(ctx);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }

    fn teardown(&self, element: &mut Box<dyn Widget>, ctx: &mut BuildCtx<'_>) {
        if let Some(view) = &self.view {
            view.teardown(element, ctx);
        }
    }
}

/// Register the app's hot-patch anchor: its `__frust_hotpatch_anchor` function
/// ([`frust_hotpatch::ANCHOR_SYMBOL`]). Until it is set, frust-hotpatch refuses every patch.
pub fn set_anchor(anchor: extern "C" fn()) {
    frust_hotpatch::set_anchor(anchor as usize);
}

/// Register `listener` to run right after a patch library is loaded and the jump table swapped.
///
/// It runs on the thread that applied the patch, not the UI thread: it must only signal (e.g. send
/// through an event-loop proxy).
pub fn set_patch_listener(listener: Arc<dyn Fn() + Send + Sync>) {
    frust_hotpatch::register_handler(listener);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

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

    /// A state type named by no other test, so its mismatch record is this test's alone.
    struct WitnessProbeState(u32);
    struct WitnessProbe;
    impl Component for WitnessProbe {
        type State = WitnessProbeState;
        fn init(&self) -> WitnessProbeState {
            WitnessProbeState(0)
        }
        fn build(&self, state: &mut WitnessProbeState) -> impl View<WitnessProbeState> {
            state.0 += 1;
            Inert
        }
    }

    fn element_type(view: &AnyView<u32>) -> std::any::TypeId {
        let mut id = 0u64;
        let mut ctx = BuildCtx::new(&mut id);
        let element = view.build(&mut ctx);
        let element: &dyn Any = &*element;
        element.type_id()
    }

    #[test]
    fn a_matching_witness_builds_as_a_direct_call_does() {
        let c = Probe;
        let mut state = 0u32;
        let direct = AnyView::new(c.build(&mut state));
        let via_seam = build_erased(&c, &mut state, SeamWitness::of::<Probe>())
            .expect("the creator's own witness matches");
        // Both builds ran the real `build` exactly once each.
        assert_eq!(state, 2);
        assert_eq!(element_type(&direct), element_type(&via_seam));
    }

    #[test]
    fn a_mismatched_witness_is_reported_and_leaves_the_state_alone() {
        let c = WitnessProbe;
        let mut state = WitnessProbeState(7);
        let own = SeamWitness::of::<WitnessProbe>();
        let stored = SeamWitness {
            size_state: own.size_state + 8,
            ..own
        };
        let err = build_erased(&c, &mut state, stored)
            .err()
            .expect("a foreign witness is refused");
        assert_eq!(state.0, 7, "build never ran");
        assert_eq!(err.type_name, type_name::<WitnessProbeState>());
        assert_eq!(err.stored, stored.to_string());
        assert_eq!(err.own, own.to_string());
        assert!(frust_hotpatch::pending_layout_mismatches().contains(&err));
        frust_hotpatch::mark_layout_mismatches_reported(&[err]);
    }

    #[test]
    fn a_refused_root_keeps_the_live_tree() {
        let mut id = 0u64;
        let mut ctx = BuildCtx::new(&mut id);
        let first = RootView::new(Ok(AnyView::new(Leaf)));
        let mut element = first.build(&mut ctx);
        let refused: RootView<u32> = RootView::new(Err(LayoutMismatch {
            type_name: "Root".into(),
            stored: "a".into(),
            own: "b".into(),
        }));
        assert_eq!(
            refused.rebuild(&first, &mut element, &mut ctx),
            ChangeFlags::NONE
        );
        let live: &dyn Any = &*element;
        assert!(live.is::<LeafWidget>(), "the stale tree stays in place");
        // Tearing a refused root down touches nothing either.
        refused.teardown(&mut element, &mut ctx);
        let live: &dyn Any = &*element;
        assert!(live.is::<LeafWidget>());
    }

    extern "C" fn test_anchor() {}

    #[test]
    fn set_anchor_registers_the_functions_address() {
        set_anchor(test_anchor);
        assert_eq!(
            frust_hotpatch::aslr_reference(),
            test_anchor as extern "C" fn() as usize
        );
    }
}
