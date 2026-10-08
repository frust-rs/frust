//! Hot-patch seam (opt-in `hotpatch` feature): every `Component::build` runs through frust-hotpatch's
//! jump table, so a `dx` patch that swaps the monomorphised call takes effect on the next rebuild.
//!
//! The seam's boundary types are layout-fixed. [`build_erased`] erases the component's view inside
//! the hot function, so the return slot is `Result<AnyView<_>, LayoutMismatch>` whatever the view
//! type is; and it takes a [`SeamWitness`] written by the image that created the state, which the
//! (possibly patched) callee compares with its own layout before it touches that state.

use std::any::type_name;
use std::cell::RefCell;
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
/// A refused frame keeps the tree already on screen: the rebuild does nothing (as a nested
/// component's skipped rebuild does) and carries the last accepted view forward, so a later
/// accepted build (after the host acknowledged the mismatch and a corrective or revert patch
/// matching the stored layout was applied) is diffed against it in place: no replace, nested
/// component state kept, the normal teardown path. Only a first build that mismatches has nothing
/// to carry: it shows an empty root, and the first accepted build then replaces that empty root.
pub struct RootView<S: 'static> {
    /// The accepted view whose tree is on screen when this is the current frame: this frame's own
    /// view, or (refused frame) the one moved in from the previous frame by `rebuild`.
    live: RefCell<Option<AnyView<S>>>,
    /// Whether this frame's build was accepted.
    accepted: bool,
}

impl<S: 'static> RootView<S> {
    /// Wrap the result of the root component's [`build_erased`].
    pub fn new(built: Result<AnyView<S>, LayoutMismatch>) -> Self {
        let accepted = built.is_ok();
        Self {
            live: RefCell::new(built.ok()),
            accepted,
        }
    }
}

impl<S: 'static> View<S> for RootView<S> {
    type Element = Box<dyn Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Box<dyn Widget> {
        match self.live.borrow().as_ref() {
            Some(view) if self.accepted => view.build(ctx),
            _ => Box::new(InertWidget),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Box<dyn Widget>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if !self.accepted {
            // Refused: leave the live tree exactly as it is, and keep the last accepted view so
            // a later accepted frame can diff against it.
            let mut live = self.live.borrow_mut();
            if live.is_none() {
                *live = prev.live.borrow_mut().take();
            }
            return ChangeFlags::NONE;
        }
        let live = self.live.borrow();
        let Some(view) = live.as_ref() else {
            return ChangeFlags::NONE;
        };
        match prev.live.borrow().as_ref() {
            Some(prev_view) => view.rebuild(prev_view, element, ctx),
            // No accepted view was ever on screen (the first build was refused), so the element
            // is the empty stand-in and there is nothing to diff against or tear down: build
            // the tree outright.
            None => {
                if ctx.has_focus() {
                    crate::event::mark_focus_orphaned();
                }
                *element = view.build(ctx);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }

    fn teardown(&self, element: &mut Box<dyn Widget>, ctx: &mut BuildCtx<'_>) {
        if let Some(view) = self.live.borrow().as_ref() {
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

    /// Counts what the seam test below must observe on the tree under the root.
    #[derive(Default)]
    struct Counts {
        built: std::cell::Cell<u32>,
        rebuilt: std::cell::Cell<u32>,
        torn: std::cell::Cell<u32>,
        dropped: std::cell::Cell<u32>,
    }

    struct CountedView(std::rc::Rc<Counts>);
    struct CountedWidget(std::rc::Rc<Counts>);
    impl Drop for CountedWidget {
        fn drop(&mut self) {
            self.0.dropped.set(self.0.dropped.get() + 1);
        }
    }
    impl Widget for CountedWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(1.0, 1.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }
    impl View<RecoveryState> for CountedView {
        type Element = CountedWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CountedWidget {
            self.0.built.set(self.0.built.get() + 1);
            CountedWidget(self.0.clone())
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut CountedWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            self.0.rebuilt.set(self.0.rebuilt.get() + 1);
            ChangeFlags::NONE
        }
        fn teardown(&self, _element: &mut CountedWidget, _ctx: &mut BuildCtx<'_>) {
            self.0.torn.set(self.0.torn.get() + 1);
        }
    }

    /// A state type named by no other test, so its mismatch record is this test's alone.
    struct RecoveryState(u32);
    struct RecoveryRoot(std::rc::Rc<Counts>);
    impl Component for RecoveryRoot {
        type State = RecoveryState;
        fn init(&self) -> RecoveryState {
            RecoveryState(0)
        }
        fn build(&self, state: &mut RecoveryState) -> impl View<RecoveryState> {
            state.0 += 1;
            CountedView(self.0.clone())
        }
    }

    fn widget_addr(element: &dyn Widget) -> *const () {
        std::ptr::from_ref(element).cast::<()>()
    }

    #[test]
    fn an_accepted_root_after_a_refused_one_diffs_the_retained_tree() {
        let counts = std::rc::Rc::new(Counts::default());
        let root = RecoveryRoot(counts.clone());
        let mut state = root.init();
        let own = SeamWitness::of::<RecoveryRoot>();
        let mut id = 0u64;
        let mut ctx = BuildCtx::new(&mut id);

        // Frame 1: accepted.
        let first = RootView::new(build_erased(&root, &mut state, own));
        let mut element = first.build(&mut ctx);
        let addr = widget_addr(&*element);
        assert_eq!((counts.built.get(), counts.dropped.get()), (1, 0));

        // Frame 2: refused (a foreign witness): the tree is untouched and one mismatch is
        // reported.
        let stored = SeamWitness {
            size_state: own.size_state + 8,
            ..own
        };
        let second = RootView::new(build_erased(&root, &mut state, stored));
        assert_eq!(state.0, 1, "the refused build never ran");
        let ours = |r: &LayoutMismatch| r.type_name == type_name::<RecoveryState>();
        let reported: Vec<_> = frust_hotpatch::pending_layout_mismatches()
            .into_iter()
            .filter(ours)
            .collect();
        assert_eq!(reported.len(), 1, "exactly one mismatch is reported");
        assert_eq!(
            second.rebuild(&first, &mut element, &mut ctx),
            ChangeFlags::NONE
        );
        assert_eq!(widget_addr(&*element), addr);
        assert_eq!(
            (
                counts.built.get(),
                counts.rebuilt.get(),
                counts.dropped.get()
            ),
            (1, 0, 0)
        );

        // The host acknowledges the mismatch; a later accepted frame diffs in place.
        frust_hotpatch::mark_layout_mismatches_reported(&reported);
        let third = RootView::new(build_erased(&root, &mut state, own));
        assert_eq!(state.0, 2);
        third.rebuild(&second, &mut element, &mut ctx);
        assert_eq!(widget_addr(&*element), addr, "same widget: no replace");
        assert_eq!(
            (
                counts.built.get(),
                counts.rebuilt.get(),
                counts.torn.get(),
                counts.dropped.get()
            ),
            (1, 1, 0, 0),
            "diffed once: no extra build, nothing disposed"
        );

        // Teardown goes through the retained view's normal path.
        third.teardown(&mut element, &mut ctx);
        assert_eq!(counts.torn.get(), 1);
        drop(element);
        assert_eq!(counts.dropped.get(), 1);
    }

    #[test]
    fn a_first_refused_root_is_replaced_by_the_first_accepted_one() {
        let counts = std::rc::Rc::new(Counts::default());
        let mut id = 0u64;
        let mut ctx = BuildCtx::new(&mut id);
        let refused: RootView<RecoveryState> = RootView::new(Err(LayoutMismatch {
            type_name: "Root".into(),
            stored: "a".into(),
            own: "b".into(),
        }));
        let mut element = refused.build(&mut ctx);
        let accepted = RootView::new(Ok(AnyView::new(CountedView(counts.clone()))));
        assert_eq!(
            accepted.rebuild(&refused, &mut element, &mut ctx),
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        );
        assert_eq!(counts.built.get(), 1);
        let live: &dyn Any = &*element;
        assert!(live.is::<CountedWidget>());
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
