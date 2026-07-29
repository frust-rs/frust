//! The generic mounting builder:
//! [`native_component`] turns a registered
//! [`NativeComponent`](crate::component::NativeComponent) plus its typed
//! `Props` into a frust [`View`], composing exactly the one `platform_view`
//! slot [`crate::api::builders`]' six controls compose by hand.
//!
//! # Why this exists
//!
//! An earlier design shipped the public trait — define a component, register
//! it — and flagged its own gap: *there was no public way to MOUNT one into a frust
//! view tree*. A third party could implement `NativeComponent` and never use
//! it. This module closes that loop, and is what makes
//!
//! ```text
//! define (impl NativeComponent) → register_component::<C>(KIND) → native_component(KIND, c, props)
//! ```
//!
//! a path from pure Rust to a real platform view, with no per-component
//! Kotlin or Swift anywhere in it.
//!
//! **Who can walk it, and how far.** An app crate cannot implement the trait
//! today: `create` has to name `jni::objects::JObject` on Android and
//! `objc2-ui-kit`'s classes on iOS *in the implementing crate*, and this
//! plugin re-exports neither FFI crate, so the practical audience today is
//! plugin authors, not app authors (the only implementor in this repo is this
//! crate's own non-default `demo-components` composite). And the view it
//! mounts is **display-only**: no production path attaches a platform
//! listener to a component-built view, so overriding
//! `NativeComponent::on_event` has no effect for it — see that method's own
//! doc for the deferred Phase 4 gap, and for the one reachability the code
//! does leave open (a *fabricated* slot id, which misroutes rather than
//! routes). `crate::component`'s module doc states both limits in full.
//!
//! # It reuses the six's plumbing rather than re-deriving it
//!
//! Everything load-bearing here is `crate::api::builders`' own, imported
//! rather than copied: the one factory [`VIEW_TYPE`], the private slot
//! counter ([`next_local_slot`], so a component and a built-in control can
//! never collide on an id), [`resolve_size`]'s no-measure sizing rule, and
//! [`placeholder`] — the refusal fallback *and* its clip wrapper. The two
//! tests that pin the refusal contract for the six
//! (`a_refused_slot_publishes_no_platform_view_frame` and its clip-wrapper
//! sibling) therefore describe this path too, and
//! [`tests::a_refused_component_slot_publishes_no_platform_view_frame`]
//! asserts it directly.
//!
//! **The six are deliberately NOT rewritten to route through here** (a
//! standing decision, unchanged): their whole wire is `params_json` decoded
//! inside an internal `NativeWidget`, while a component's props are typed
//! Rust values staged beside the wire — one builder cannot be both without
//! changing six shipped files whose "behaviour must not change" guarantee
//! rests on their code not moving. Sharing the *helpers* buys the reuse
//! without the risk.
//!
//! # What a component's builder does NOT carry
//!
//! No theme folding (`crate::api::theme`'s ladder L2 is the six controls'
//! wire-side mechanism; a component's props are typed, so an app reads
//! `use_context::<Theme>()` itself and puts whatever it wants in them), and no
//! event-callback registration (`crate::api::signals`' three wrappers ride the
//! internal `EventPayload` channel, which a public component was meant to
//! bypass by handling `on_event` itself — see `crate::component`'s bridge
//! doc). The second omission is currently a **dead end rather than a
//! delegation**: no production path attaches a listener to a component-built
//! view either, so nothing arrives at `on_event` to be handled (above).

use std::rc::Rc;

use frust::{ResolvedSurfaceMode, on_cleanup, platform_view, resolved_surface_mode};
use frust_core::{
    AnyView, BuildCtx, ChangeFlags, Component, ComponentWidget, View, any, component,
};

// Aliased: `frust_core`'s own `component` view-fn (imported above, used by the
// `View<Outer>` delegate at the bottom) already owns that name here.
use crate::component as component_api;
use crate::component::NativeComponent;
use crate::registry::SlotId;

use super::builders::{VIEW_TYPE, next_local_slot, placeholder, resolve_size};

/// Mount the registered [`NativeComponent`] `C` as one native slot, driven by
/// `props`.
///
/// `kind` must be the same string the component was registered under
/// ([`register_component`](crate::component::register_component)); a slot
/// naming a kind nothing registered is reported dead by the runtime rather
/// than rendering. `component` is the value the app constructs fresh every
/// rebuild — the value [`NativeComponent::on_event`] would run against, though
/// no production path dispatches to it in this build (the module doc's
/// *display-only*) — and `props` is the typed create/update payload the
/// runtime diffs with `PartialEq` before any FFI crossing.
///
/// ```ignore
/// register_component::<Gauge>("gauge");          // once, at app init
/// // …then, in a rebuild:
/// native_component("gauge", Gauge::new(), GaugeProps { value: 42 })
///     .size(160.0, 48.0)
///     .interactive()
/// ```
///
/// The returned view is a [`View<Outer>`](View) for every outer app-state
/// type, exactly like the six built-in builders.
pub fn native_component<C: NativeComponent>(
    kind: &'static str,
    component: C,
    props: C::Props,
) -> NativeComponentView<C> {
    NativeComponentView {
        kind,
        // `Rc` so a rebuild costs a refcount bump rather than a deep clone,
        // and so the public trait needs no `Clone` bound of its own.
        component: Rc::new(component),
        props,
        size: None,
        interactive: false,
        semantics_label: None,
    }
}

/// One mounted [`NativeComponent`] — build it with [`native_component`].
pub struct NativeComponentView<C: NativeComponent> {
    kind: &'static str,
    component: Rc<C>,
    props: C::Props,
    size: Option<(f64, f64)>,
    interactive: bool,
    semantics_label: Option<String>,
}

/// Hand-written (rather than derived) so the builder is `Clone` whatever `C`
/// is: `#[derive(Clone)]` would demand `C: Clone` + `C::Props: Clone`, and
/// only the second is a trait bound this crate can rely on.
impl<C: NativeComponent> Clone for NativeComponentView<C> {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            component: Rc::clone(&self.component),
            props: self.props.clone(),
            size: self.size,
            interactive: self.interactive,
            semantics_label: self.semantics_label.clone(),
        }
    }
}

impl<C: NativeComponent> NativeComponentView<C> {
    /// Explicit slot size — see `crate::api::builders`' `resolve_size` doc for
    /// the no-call fallback (fill the parent; there is no measure step in v1,
    /// and a native subtree's own content size is invisible to frust by
    /// design).
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Mark the slot interactive, so the host routes touches to the native
    /// view (and the differ collects the frust chrome overlapping it as
    /// input shields). Off by default: a display-only component wants
    /// neither.
    ///
    /// **A `platform_view` slot never receives `Widget::event`**
    /// (`docs/CODE_STANDARDS.md`'s Platform-View Conventions) — the platform
    /// owns this input end to end, and it would surface through
    /// [`NativeComponent::on_event`], never `EventCtx`. In this build it
    /// surfaces nowhere in Rust: the touch reaches the native view (a button
    /// highlights, a scroll view scrolls) but no production path attaches a
    /// listener to report it back, so marking a component interactive buys
    /// platform-side behaviour only (the module doc's *display-only*).
    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// The slot's frust-side accessibility label. A native subtree already
    /// gets the platform's own a11y traversal for free (the platform owns it);
    /// this labels the *slot* for frust's semantics pass, which publishes
    /// nothing for an unlabelled one.
    pub fn semantics_label(mut self, label: impl Into<String>) -> Self {
        self.semantics_label = Some(label.into());
        self
    }

    /// [`Component::build`]'s real body, with `mode` threaded explicitly so a
    /// test can force the [`ResolvedSurfaceMode::RefusedTranslucent`] branch
    /// without touching the process-global resolved-mode slot (whose writer is
    /// pinned to the two shells' own FFI glue) — the same shape every builder
    /// in `crate::api::builders` uses.
    fn build_with_mode(&self, slot: SlotId, mode: ResolvedSurfaceMode) -> AnyView<SlotId> {
        if mode.translucency_refused() {
            // Before publishing anything: a refused slot mounts no native
            // view, so it must stage no props and trigger no factory lookup
            // either (the six take exactly this branch, in exactly this
            // order).
            return placeholder(self.size, self.kind);
        }
        // Stage this rebuild's component value and props beside the wire, and
        // carry only the generation it answers with — which changes if and
        // only if the props actually changed, so the differ emits an
        // `UpdateParams` exactly then (`crate::component`'s *Props travel
        // beside the wire*).
        let generation =
            component_api::publish(slot, Rc::clone(&self.component), self.props.clone());
        let params = component_api::component_params(self.kind, slot, generation);
        let view = platform_view(VIEW_TYPE).params_json(params);
        let view = if self.interactive {
            view.interactive()
        } else {
            view
        };
        let view = match self.semantics_label.clone() {
            Some(label) => view.semantics_label(label),
            None => view,
        };
        any(resolve_size(self.size, view))
    }
}

impl<C: NativeComponent> Component for NativeComponentView<C> {
    type State = SlotId;

    fn init(&self) -> SlotId {
        let slot = next_local_slot();
        // The staging table's reaper (`crate::component::forget`), tied to
        // this Component's own lifetime rather than to the native
        // create/dispose lifecycle — the same leak shape, one table over:
        // a culled slot's dispose resolves by native-view identity and never
        // names a slot id, so disposal alone would strand the staged entry.
        // `init` runs exactly once, under this component's own `Owner`, so
        // this fires exactly once when that owner disposes.
        //
        // No `forget_pending_callback` reaper beside it (unlike
        // `NativeButtonView::init`): the generic path never calls
        // `set_callback` at all — a public component handles its own events
        // (`crate::component::Bridge::on_event` returns `None`
        // unconditionally), so it never has a `pending_callbacks` entry to
        // reap.
        on_cleanup(move || component_api::forget(slot));
        slot
    }

    fn build(&self, state: &mut SlotId) -> AnyView<SlotId> {
        self.build_with_mode(*state, resolved_surface_mode())
    }
}

/// `View<Outer>` for every `Outer`, by delegating to `frust_core::component`
/// — the generic counterpart of `crate::api::builders`' `impl_native_view!`
/// macro, hand-written here because that macro takes a concrete type and this
/// one carries a parameter. Same contract, same rationale: each call clones
/// `self`/`prev` into a throwaway `ComponentView` (cheap — an `Rc` bump plus
/// the props), and it is correct because `ComponentView::rebuild` never reads
/// its `prev` argument, always re-running `Component::build` against the
/// retained `State`.
impl<C: NativeComponent, Outer: 'static> View<Outer> for NativeComponentView<C> {
    type Element = ComponentWidget<Self>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        <frust_core::ComponentView<Self> as View<Outer>>::build(&component(self.clone()), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        <frust_core::ComponentView<Self> as View<Outer>>::rebuild(
            &component(self.clone()),
            &component(prev.clone()),
            element,
            ctx,
        )
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        <frust_core::ComponentView<Self> as View<Outer>>::teardown(
            &component(self.clone()),
            element,
            ctx,
        );
    }
}

// Gated on the host arm, not merely on `test` (the gate `crate::component`'s
// own tests carry): these define a real component through the public trait,
// whose `create` builds against the host stand-in context a platform build
// replaces with the device-only types.
#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use frust_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, Widget};
    use kurbo::{Point, Size};
    use peniko::Color;

    use super::*;
    use crate::component::{ComponentCtx, NativeRoot, register_component};
    use crate::runtime::{DisposeOutcome, NativeCtx as PlatformCtx, with_runtime};

    /// A component defined **outside the six**, implementing nothing but the
    /// public trait — a third-party crate's shape exactly.
    struct Meter;

    #[derive(Clone, Debug, PartialEq)]
    struct MeterProps {
        value: i32,
    }

    const METER_KIND: &str = "test-meter";

    impl NativeComponent for Meter {
        type Props = MeterProps;
        type State = u64;

        fn create(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            props: &Self::Props,
        ) -> Option<(NativeRoot, Self::State)> {
            let identity = 1_000;
            ctx.record(format!("meter create {identity} = {}", props.value));
            Some((ctx.root(identity)?, identity))
        }

        fn update(
            &self,
            ctx: &mut ComponentCtx<'_, '_, '_>,
            state: &mut Self::State,
            _old: &Self::Props,
            new: &Self::Props,
        ) {
            ctx.record(format!("meter update {state} = {}", new.value));
        }
    }

    /// A minimal `PaintScene` — only `fill_rect`/`draw_text` have no default
    /// (mirroring `crate::api::builders`' own test scene).
    #[derive(Default)]
    struct NullScene;

    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    fn build_any<State: 'static>(view: AnyView<State>) -> Box<dyn Widget> {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    /// Paint `view` at a 120x44 slot and return whatever platform-view frames
    /// it published.
    fn frames_of(view: AnyView<SlotId>) -> Vec<frust_core::widget::PlatformViewFrame> {
        let mut element = build_any(view);
        let mut lctx = LayoutCtx::new();
        element.layout(&mut lctx, &BoxConstraints::tight(Size::new(120.0, 44.0)));
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 44.0));
        element.paint(&mut pctx, &mut scene);
        pctx.take_platform_views()
    }

    #[test]
    fn a_component_defined_outside_the_six_mounts_and_publishes_exactly_one_slot() {
        // The acceptance bar: define → register → mount → create,
        // end to end, through the public surface alone.
        assert!(register_component::<Meter>(METER_KIND));

        let mounted = native_component(METER_KIND, Meter, MeterProps { value: 42 })
            .size(120.0, 44.0)
            .interactive()
            .semantics_label("meter");
        let frames = frames_of(mounted.build_with_mode(31, ResolvedSurfaceMode::Opaque));

        assert_eq!(frames.len(), 1, "one component == one slot");
        assert_eq!(frames[0].view_type, VIEW_TYPE);
        assert!(frames[0].interactive);
        assert_eq!(
            frames[0].rect,
            kurbo::Rect::from_origin_size(Point::ZERO, Size::new(120.0, 44.0)),
            "the slot took the size the builder declared"
        );

        // The params the mount actually published are the ones the runtime
        // dispatches on — feed the very same string back in, which is what
        // the host's post-frame poll does a frame later.
        let params = frames[0].params_json.clone();
        assert!(
            params.contains("\"__frustControl\":\"test-meter\"") && params.contains("31"),
            "{params}"
        );

        let mut calls = Vec::new();
        {
            let mut ctx = PlatformCtx::new(&mut calls);
            with_runtime(|runtime| {
                assert_eq!(runtime.create(&mut ctx, &params).unwrap(), 31);
                assert_eq!(runtime.live_count(), 1);
                assert_eq!(runtime.dispose_slot(&mut ctx, 31), DisposeOutcome::Disposed);
                assert_eq!(runtime.live_count(), 0);
            })
            .expect("the thread's runtime");
        }
        assert_eq!(calls, vec!["meter create 1000 = 42".to_string()]);

        component_api::forget(31);
        assert_eq!(component_api::staged_count(), 0);
    }

    #[test]
    fn an_unchanged_rebuild_republishes_byte_identical_params() {
        // The diff gate's wire half: only a real props change may move the
        // generation, or the differ would emit an `UpdateParams` every frame
        // and the whole zero-FFI property would be forfeit.
        register_component::<Meter>(METER_KIND);

        let first = native_component(METER_KIND, Meter, MeterProps { value: 7 })
            .build_with_mode(32, ResolvedSurfaceMode::Opaque);
        let again = native_component(METER_KIND, Meter, MeterProps { value: 7 })
            .build_with_mode(32, ResolvedSurfaceMode::Opaque);
        let changed = native_component(METER_KIND, Meter, MeterProps { value: 8 })
            .build_with_mode(32, ResolvedSurfaceMode::Opaque);

        let params = |view| frames_of(view).remove(0).params_json;
        let (first, again, changed) = (params(first), params(again), params(changed));
        assert_eq!(
            first, again,
            "an unchanged rebuild changes nothing on the wire"
        );
        assert_ne!(again, changed);

        component_api::forget(32);
    }

    #[test]
    fn a_refused_component_slot_publishes_no_platform_view_frame() {
        // The refusal contract the six are pinned to
        // (`a_refused_slot_publishes_no_platform_view_frame` and its
        // clip-wrapper sibling), asserted for the generic path — which
        // degrades through the very same `placeholder`.
        register_component::<Meter>(METER_KIND);

        let mounted =
            native_component(METER_KIND, Meter, MeterProps { value: 42 }).size(120.0, 44.0);
        let refused = mounted.build_with_mode(33, ResolvedSurfaceMode::RefusedTranslucent);

        let mut element = build_any(refused);
        let mut scene = NullScene;
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(120.0, 44.0));
        element.paint(&mut pctx, &mut scene);
        assert!(
            pctx.take_platform_views().is_empty(),
            "a refused slot must render no native platform_view frame"
        );
        assert_eq!(
            component_api::staged_count(),
            0,
            "a refused slot stages no props either — there is nothing to create"
        );
    }
}
