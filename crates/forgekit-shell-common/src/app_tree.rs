//! [`AppTree`]: the type-erasure that lets a single non-generic native handle
//! drive any app's `State`/`app_logic`/`View`.
//!
//! Every platform shell stores its running app as a `Box<dyn AppTree>` behind an
//! opaque handle, so the FFI-exported entry points (which can't be generic) stay
//! non-generic while still driving a concrete app. This is the one seam that
//! keeps the shell runtime widget-agnostic — apps bring their own view types in
//! through the shell's app-binding macro.

use std::any::Any;

use forgekit_core::accesskit;
use forgekit_core::anim::FrameTime;
use forgekit_core::event::{EditingState, EventOutcome, ImeEvent, ImeState, InputEvent};
use forgekit_core::insets::WindowInsets;
use forgekit_core::view::{ChangeFlags, View};
use forgekit_core::{PaintOutcome, PaintScene, RenderRoot, SemanticsUpdate};
use kurbo::Size;

/// Type-erased app tree: the one seam that lets a shell's native handle stay
/// non-generic while still driving a concrete `State`/`app_logic`/`View`.
///
/// Mirrors the desktop facade's erasure approach (a stored generic behind a
/// non-generic driver): a platform shell's generated `extern` entry points can't
/// be generic, so the shell's app-binding macro instantiates [`new_boxed_app`]
/// with the app's types and stores the result as a `Box<dyn AppTree>` inside the
/// handle.
pub trait AppTree {
    /// Re-run `app_logic` and reconcile the retained tree (spec §5).
    fn rebuild(&mut self);

    /// Take (and clear) the layout/paint dirtiness accumulated since the last
    /// call (delegates to [`RenderRoot::take_change_flags`]).
    ///
    /// The frame-gate seam (spec §14 phase 7): a shell drives the layout-skip
    /// decision off this — run [`AppTree::rebuild`], then run [`AppTree::layout`]
    /// only if the drained flags [`ChangeFlags::needs_layout`] (or it's the
    /// first frame, or the surface resized), then always [`AppTree::paint`].
    /// The `set_theme ⇒ LAYOUT|PAINT` contract keeps `Text`'s layout-baked
    /// theme color correct across a bare theme swap (see
    /// `crate::frame_gate`'s module docs). Draining is the caller's commitment
    /// to act on the flags this frame; use
    /// [`AppTree::has_pending_change_flags`] to peek without draining when
    /// gathering [`crate::FrameInputs`] for a frame that may be skipped.
    fn take_change_flags(&mut self) -> ChangeFlags;

    /// Non-draining peek at whether any layout/paint dirtiness is pending
    /// (delegates to [`RenderRoot::has_pending_change_flags`]).
    ///
    /// The frame gate reads this as its `change_flags_pending`
    /// [`crate::FrameInputs`] entry *before* deciding, so a frame it skips
    /// leaves the flags intact for the next frame that runs to drain via
    /// [`AppTree::take_change_flags`].
    fn has_pending_change_flags(&self) -> bool;

    /// Whether a captured pointer gesture is currently in flight (delegates to
    /// [`RenderRoot::is_pointer_captured`]).
    ///
    /// A frame-gate input (spec §14 phase 7): a mid-drag captured widget may
    /// track/animate the pointer, so the mobile shells feed this into
    /// [`crate::FrameInputs::pointer_capture_active`] to keep producing frames
    /// while a gesture is live rather than skipping it.
    fn is_pointer_captured(&self) -> bool;

    /// Whether some widget in the tree currently holds keyboard/IME focus
    /// (delegates to [`RenderRoot::is_focus_active`]).
    ///
    /// A frame-gate input (spec §14 phase 7): a focused field's caret/selection
    /// chrome may need repainting, so the mobile shells feed this into
    /// [`crate::FrameInputs::focus_or_ime_active`].
    fn is_focus_active(&self) -> bool;

    /// Lay the tree out against a logical (density-independent) size, threading
    /// the shell-owned `TextContext` down type-erased (spec §10.3).
    fn layout(&mut self, logical: Size, text_ctx: &mut dyn Any);
    /// Paint the tree into a scene builder at the shell-provided `frame_time`.
    ///
    /// `frame_time` is the shell's shared monotonic clock for this frame, threaded
    /// through to every animating widget as [`forgekit_core::widget::PaintCtx::frame_time`]
    /// (spec §8: time enters from the shell, never `Instant::now()` inside the
    /// framework).
    ///
    /// Returns a [`PaintOutcome`] whose `needs_frame` is set when a widget
    /// advanced animation state during paint and wants another frame (spec's v1
    /// animation seam). The desktop shell honors it with `window.request_redraw()`;
    /// the mobile shells' continuous Choreographer/`CADisplayLink` loops already
    /// produce the next frame and may ignore it.
    fn paint(&mut self, scene: &mut dyn PaintScene, frame_time: FrameTime) -> PaintOutcome;
    /// Deliver one platform input event to the retained tree (spec §9).
    ///
    /// Delegates to [`RenderRoot::event`], threading the erased `State` the same
    /// way [`AppTree::rebuild`] does. The returned [`EventOutcome`] carries
    /// `needs_redraw`, which the shell honours by scheduling a frame: the desktop
    /// shell calls `window.request_redraw()`, while the mobile shells' continuous
    /// Choreographer/`CADisplayLink` loops already produce the next frame. The
    /// event pass itself never rebuilds or repaints (see [`RenderRoot::event`]).
    fn event(&mut self, event: &InputEvent) -> EventOutcome;

    /// Apply a whole editing state pushed by the platform IME (the mobile
    /// state-sync path), routed to the focused widget as an
    /// [`InputEvent::Ime`]`(`[`ImeEvent::ApplyEditingState`]`)`.
    ///
    /// `state`'s selection/composing indices are UTF-16 code-unit based (the
    /// platform-native unit); the focused widget / `forgekit-text` converts them
    /// to Rust byte offsets. Returns the same [`EventOutcome`] as [`AppTree::event`].
    fn ime_apply(&mut self, state: EditingState) -> EventOutcome;

    /// The IME surface the focused widget published, for the shell to drive the
    /// platform input method. Delegates to [`RenderRoot::ime_state`]; `None` when
    /// nothing is focused or no IME surface was published.
    fn ime_state(&self) -> Option<ImeState>;

    /// Store the app's active theme, threaded into every subsequent
    /// layout/paint pass (delegates to [`RenderRoot::set_theme`]).
    ///
    /// The theme is **type-erased** (`Box<dyn Any>`) so `forgekit-shell-common`
    /// stays free of a `forgekit-theme` dependency (it compiles everywhere with
    /// no unsafe/FFI — see `docs/ARCHITECTURE.md`). The concrete `Theme` is
    /// boxed by the shell that owns the appearance state; the mobile shells wire
    /// this up in a later task (08). Re-boxing on a live appearance change
    /// replaces the stored theme.
    fn set_theme(&mut self, theme: Box<dyn Any>);

    /// Store the window's insets ([`WindowInsets`]), threaded into every
    /// subsequent layout/paint pass (delegates to [`RenderRoot::set_insets`]).
    ///
    /// A shell reads the platform's per-edge occlusion (Android `WindowInsets`,
    /// iOS `safeAreaInsets` + keyboard frame), converts device px to **logical**
    /// px at the FFI boundary (see
    /// [`crate::ffi_support::logical_insets`](crate::logical_insets)), and pushes
    /// the result here; a `SafeArea` widget then insets by
    /// [`WindowInsets::padding`]. `WindowInsets` is a concrete core-owned type
    /// (only `f64` scalars), so this needs no `Box<dyn Any>` erasure — unlike
    /// [`AppTree::set_theme`].
    ///
    /// Defaulted to a **no-op** so existing [`AppTree`] impls compile unchanged;
    /// the concrete tree overrides it to forward to [`RenderRoot::set_insets`].
    /// The `set_insets ⇒ LAYOUT | PAINT` dirty contract keeps a `SafeArea`'s
    /// layout-time inset resolution correct under the mobile layout-skip gate,
    /// exactly like `set_theme` (see [`crate::frame_gate`]'s module docs).
    fn set_insets(&mut self, _insets: WindowInsets) {}

    /// Collect the accessibility tree for the current frame (spec §9, phase-6d),
    /// for a shell to push into its platform `accesskit_*` adapter. Delegates to
    /// [`RenderRoot::semantics`]; must run **after** [`AppTree::layout`] so node
    /// bounds are valid.
    fn semantics(&mut self) -> SemanticsUpdate;

    /// The current semantics generation, bumped whenever a rebuild/theme swap
    /// could have changed the tree (delegates to
    /// [`RenderRoot::semantics_generation`]). A shell compares it to skip
    /// re-pushing an unchanged tree — see [`AppTree::semantics_if_changed`].
    fn semantics_generation(&self) -> u64;

    /// Pull a fresh [`SemanticsUpdate`] only if the tree may have changed since
    /// generation `last_seen` (delegates to [`RenderRoot::semantics_if_changed`]),
    /// so a shell's adapter push runs only when something changed.
    fn semantics_if_changed(&mut self, last_seen: u64) -> Option<SemanticsUpdate>;

    /// Perform a platform accessibility action delivered by the shell's
    /// `accesskit_*` adapter (an `ActionRequest`), threading the erased `State`
    /// the same way [`AppTree::event`] does (delegates to
    /// [`RenderRoot::perform_accessibility_action`]).
    ///
    /// `node_id` is the raw accesskit id the adapter reported; `action` is the
    /// requested [`accesskit::Action`]. Returns the same [`EventOutcome`] as
    /// [`AppTree::event`] — its `needs_redraw` tells the shell whether to schedule
    /// a frame. An unknown node or unmodelled action is a benign no-op.
    fn perform_accessibility_action(
        &mut self,
        node_id: u64,
        action: accesskit::Action,
    ) -> EventOutcome;
}

/// Concrete [`AppTree`] holding one app's state, logic and retained root.
struct ErasedApp<State: 'static, Logic, V: View<State>> {
    state: State,
    logic: Logic,
    root: RenderRoot<State, V>,
}

impl<State, Logic, V> AppTree for ErasedApp<State, Logic, V>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    fn rebuild(&mut self) {
        // app_logic is cheap by construction (spec §5). The returned flags are
        // merged into `RenderRoot::pending` and surfaced to the shell's frame
        // gate via `take_change_flags`/`has_pending_change_flags` below.
        let _flags = self.root.rebuild(&mut self.logic, &mut self.state);
    }

    fn take_change_flags(&mut self) -> ChangeFlags {
        self.root.take_change_flags()
    }

    fn has_pending_change_flags(&self) -> bool {
        self.root.has_pending_change_flags()
    }

    fn is_pointer_captured(&self) -> bool {
        self.root.is_pointer_captured()
    }

    fn is_focus_active(&self) -> bool {
        self.root.is_focus_active()
    }

    fn layout(&mut self, logical: Size, text_ctx: &mut dyn Any) {
        self.root.layout_with_text(logical, text_ctx);
    }

    fn paint(&mut self, scene: &mut dyn PaintScene, frame_time: FrameTime) -> PaintOutcome {
        self.root.paint(scene, frame_time)
    }

    fn event(&mut self, event: &InputEvent) -> EventOutcome {
        self.root.event(&mut self.state, event)
    }

    fn ime_apply(&mut self, state: EditingState) -> EventOutcome {
        self.root.event(
            &mut self.state,
            &InputEvent::Ime(ImeEvent::ApplyEditingState(state)),
        )
    }

    fn ime_state(&self) -> Option<ImeState> {
        self.root.ime_state()
    }

    fn set_theme(&mut self, theme: Box<dyn Any>) {
        self.root.set_theme(theme);
    }

    fn set_insets(&mut self, insets: WindowInsets) {
        self.root.set_insets(insets);
    }

    fn semantics(&mut self) -> SemanticsUpdate {
        self.root.semantics()
    }

    fn semantics_generation(&self) -> u64 {
        self.root.semantics_generation()
    }

    fn semantics_if_changed(&mut self, last_seen: u64) -> Option<SemanticsUpdate> {
        self.root.semantics_if_changed(last_seen)
    }

    fn perform_accessibility_action(
        &mut self,
        node_id: u64,
        action: accesskit::Action,
    ) -> EventOutcome {
        self.root
            .perform_accessibility_action(&mut self.state, accesskit::NodeId(node_id), action)
    }
}

/// Erase an app's `State`/`app_logic` into a `Box<dyn AppTree>`, building the
/// initial `State` from a caller-supplied factory rather than a ready-made
/// value.
///
/// This is the one construction path: [`new_boxed_app`] is a thin wrapper over
/// this function that hands in a closure returning an already-built `state`.
/// The factory form lets an entry macro (e.g. a future `Component::init`
/// binding) construct `State` itself from inside the closure instead of
/// requiring the caller to build a value up front.
pub fn new_boxed_app_with<State, Logic, V, F>(state_init: F, app_logic: Logic) -> Box<dyn AppTree>
where
    F: FnOnce() -> State,
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    Box::new(ErasedApp {
        state: state_init(),
        logic: app_logic,
        root: RenderRoot::new(),
    })
}

/// Erase an app's `State`/`app_logic` into a `Box<dyn AppTree>`.
///
/// Called by a shell's app-binding macro (e.g. `forgekit::android_app!`) from its
/// generated init entry point; kept here (not in the macro) so the erasure and
/// the trait live together and the macro stays a thin shim.
pub fn new_boxed_app<State, Logic, V>(state: State, logic: Logic) -> Box<dyn AppTree>
where
    State: 'static,
    V: View<State>,
    Logic: FnMut(&mut State) -> V + 'static,
{
    new_boxed_app_with(move || state, logic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::layout::BoxConstraints;
    use forgekit_core::view::{BuildCtx, ChangeFlags};
    use forgekit_core::widget::{LayoutCtx, PaintCtx, Widget};
    use kurbo::Size;

    /// A state type with no `Default` impl — the only way it can be
    /// constructed is through the factory closure passed to
    /// [`new_boxed_app_with`], proving the seam actually threads the
    /// factory's output through rather than falling back to some default.
    struct NonDefaultState {
        label: &'static str,
    }

    struct StubWidget;
    impl Widget for StubWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::ZERO
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    struct StubView;
    impl View<NonDefaultState> for StubView {
        type Element = StubWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> StubWidget {
            StubWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut StubWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    /// A minimal [`AppTree`] that overrides *nothing* optional — used to prove
    /// [`AppTree::set_insets`]'s default no-op lets a pre-insets impl compile and
    /// be driven without panicking. Every other method is unreachable in the test
    /// (only `set_insets`, the defaulted method, is called), so they're stubbed
    /// with `unimplemented!()`.
    struct MinimalTree;
    impl AppTree for MinimalTree {
        fn rebuild(&mut self) {
            unimplemented!()
        }
        fn take_change_flags(&mut self) -> ChangeFlags {
            unimplemented!()
        }
        fn has_pending_change_flags(&self) -> bool {
            unimplemented!()
        }
        fn is_pointer_captured(&self) -> bool {
            unimplemented!()
        }
        fn is_focus_active(&self) -> bool {
            unimplemented!()
        }
        fn layout(&mut self, _logical: Size, _text_ctx: &mut dyn Any) {
            unimplemented!()
        }
        fn paint(&mut self, _scene: &mut dyn PaintScene, _frame_time: FrameTime) -> PaintOutcome {
            unimplemented!()
        }
        fn event(&mut self, _event: &InputEvent) -> EventOutcome {
            unimplemented!()
        }
        fn ime_apply(&mut self, _state: EditingState) -> EventOutcome {
            unimplemented!()
        }
        fn ime_state(&self) -> Option<ImeState> {
            unimplemented!()
        }
        fn set_theme(&mut self, _theme: Box<dyn Any>) {
            unimplemented!()
        }
        fn semantics(&mut self) -> SemanticsUpdate {
            unimplemented!()
        }
        fn semantics_generation(&self) -> u64 {
            unimplemented!()
        }
        fn semantics_if_changed(&mut self, _last_seen: u64) -> Option<SemanticsUpdate> {
            unimplemented!()
        }
        fn perform_accessibility_action(
            &mut self,
            _node_id: u64,
            _action: accesskit::Action,
        ) -> EventOutcome {
            unimplemented!()
        }
        // set_insets deliberately NOT overridden — exercises the default no-op.
    }

    #[test]
    fn app_tree_set_insets_defaults_to_no_op() {
        use forgekit_core::insets::EdgeInsets;
        let mut tree = MinimalTree;
        // Compiles (the default impl exists) and is a benign no-op — a pre-insets
        // `AppTree` impl is unaffected.
        tree.set_insets(WindowInsets::default());
        tree.set_insets(WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            EdgeInsets::ZERO,
        ));
    }

    #[test]
    fn new_boxed_app_with_uses_factory_produced_state() {
        let mut app = new_boxed_app_with(
            || NonDefaultState {
                label: "from-factory",
            },
            |state: &mut NonDefaultState| {
                assert_eq!(state.label, "from-factory");
                StubView
            },
        );
        // Drive one rebuild so `app_logic` actually observes the factory-built
        // state (it's a closure param above, but this also exercises the
        // AppTree seam end-to-end rather than just constructing the box).
        app.rebuild();
    }

    // --- Layout-skip contract (phase 7 frame gate) --------------------------
    //
    // The seam tasks 17/18 drive: rebuild -> (layout iff needs_layout / first
    // frame / resize) -> paint. This proves the `set_theme => LAYOUT|PAINT`
    // correctness anchor: a widget that BAKES its themed value at LAYOUT time
    // (like `Text`'s glyph color) relayouts on a bare theme swap, while a
    // no-change frame skips layout yet still paints the last-baked value.

    use std::cell::Cell;

    use forgekit_scene::{Command, Scene, SceneBuilder};

    /// A type-erased "theme" carrying one scalar the baking widget reads at
    /// layout time — stands in for `forgekit_theme::Theme` (the seam needs no
    /// concrete theme type to be exercised).
    struct BakeTheme {
        value: f64,
    }

    /// Bakes the threaded theme's scalar into `baked` at LAYOUT time and
    /// merely replays it at PAINT time (the `Text` layout-baked-color model).
    /// `layouts` counts how many times layout actually ran.
    struct BakeWidget {
        layouts: Rc<Cell<u32>>,
        baked: f64,
    }
    impl Widget for BakeWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.layouts.set(self.layouts.get() + 1);
            // Bake the themed value now; paint only replays it.
            self.baked = ctx.theme_as::<BakeTheme>().map(|t| t.value).unwrap_or(0.0);
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            // Encode the baked scalar in the painted rect's width so a
            // recording `Scene` reads it back without naming a color type;
            // the color itself comes from a theme constructor (type inferred,
            // so this crate needs no `peniko` dependency).
            let color = forgekit_theme::ColorScheme::m3_baseline_light().primary;
            scene.fill_rect(ctx.origin(), Size::new(self.baked, 1.0), color);
        }
    }

    struct BakeView {
        layouts: Rc<Cell<u32>>,
    }
    impl View<()> for BakeView {
        type Element = BakeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BakeWidget {
            BakeWidget {
                layouts: self.layouts.clone(),
                baked: 0.0,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut BakeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    /// Read back the width of the single `FillRect` the `BakeWidget` painted —
    /// the baked scalar the recording `Scene` captured.
    fn painted_baked(scene: &Scene) -> f64 {
        scene
            .commands()
            .iter()
            .find_map(|c| match c {
                Command::FillRect { rect, .. } => Some(rect.width()),
                _ => None,
            })
            .expect("BakeWidget paints exactly one FillRect")
    }

    /// Drive one shell-style frame through the `AppTree` seam: rebuild, then
    /// layout ONLY when the drained flags need it (or `force_layout` for the
    /// first frame / a resize), then always paint into a fresh `Scene`.
    /// Returns the painted baked value.
    fn drive_frame(app: &mut dyn AppTree, force_layout: bool) -> f64 {
        app.rebuild();
        let flags = app.take_change_flags();
        if force_layout || flags.needs_layout() {
            app.layout(Size::new(100.0, 100.0), &mut ());
        }
        let mut scene = Scene::new();
        let mut builder = SceneBuilder::new(&mut scene);
        app.paint(&mut builder, FrameTime::ZERO);
        painted_baked(&scene)
    }

    #[test]
    fn layout_skips_without_flags_but_relayouts_on_theme_swap() {
        let layouts = Rc::new(Cell::new(0u32));
        let lc = layouts.clone();
        let mut app: Box<dyn AppTree> = new_boxed_app_with(
            || (),
            move |_s: &mut ()| BakeView {
                layouts: lc.clone(),
            },
        );

        // Frame 1 (first frame): theme set, layout must run and bake value 5.
        app.set_theme(Box::new(BakeTheme { value: 5.0 }));
        let painted = drive_frame(&mut *app, true);
        assert_eq!(layouts.get(), 1, "first frame lays out");
        assert_eq!(painted, 5.0, "baked the theme value at layout");

        // Frame 2 (steady, no change): rebuild yields no flags -> layout
        // SKIPPED, but paint still replays the last-baked value correctly.
        let painted = drive_frame(&mut *app, false);
        assert_eq!(layouts.get(), 1, "an unchanged frame skips layout");
        assert_eq!(painted, 5.0, "paint-only frame still correct");

        // Frame 3: a bare theme swap marks LAYOUT|PAINT pending, so layout
        // RUNS again and re-bakes — the Text-color correctness anchor.
        app.set_theme(Box::new(BakeTheme { value: 9.0 }));
        let painted = drive_frame(&mut *app, false);
        assert_eq!(
            layouts.get(),
            2,
            "a theme swap forces relayout even with no view change"
        );
        assert_eq!(painted, 9.0, "re-baked the new theme value");

        // Frame 4 (steady again): layout skipped, paints the new baked value.
        let painted = drive_frame(&mut *app, false);
        assert_eq!(
            layouts.get(),
            2,
            "unchanged frame after the swap skips layout"
        );
        assert_eq!(painted, 9.0, "still correct with the swapped value");
    }

    // --- Root-owner regression test (review F1) -----------------------------
    //
    // A root component's `init`/`build` run under the shell's ROOT `Owner`
    // (`Owner::with`, the way `create_handle`/`forgekit::run` now wrap
    // `new_boxed_app_with` + the initial rebuild). Without an ambient owner,
    // `provide_context` in the root's `init` silently no-ops and a nested
    // component's `use_context` returns `None`. This drives that exact path and
    // asserts the context resolves through the owner chain.

    use std::cell::RefCell;
    use std::rc::Rc;

    use forgekit_core::component::{Component, component};
    use forgekit_core::view::{AnyView, any};
    use reactive_graph::owner::{Owner, provide_context, use_context};

    /// A `use_context` sink shared with the nested component's `build`.
    type Sink = Rc<RefCell<Option<u32>>>;

    /// The context value the root provides in `init` and the nested component
    /// reads back in `build`.
    #[derive(Clone, Copy)]
    struct ProvidedCtx(u32);

    /// A view leaf usable under any state — the nested component's `build`
    /// result once it has recorded the resolved context.
    struct StubLeaf;
    impl<S: 'static> View<S> for StubLeaf {
        type Element = StubWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> StubWidget {
            StubWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut StubWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    /// Root component: `provide_context`s a value in `init`, mounts a nested
    /// component in `build`.
    #[derive(Clone)]
    struct RootComp {
        sink: Sink,
    }
    impl Component for RootComp {
        type State = ();
        fn init(&self) {
            provide_context(ProvidedCtx(42));
        }
        fn build(&self, _state: &mut ()) -> AnyView<()> {
            any(component(ChildComp {
                sink: self.sink.clone(),
            }))
        }
    }

    /// Nested component: reads the root-provided context in `build` and records
    /// what it resolved to.
    #[derive(Clone)]
    struct ChildComp {
        sink: Sink,
    }
    impl Component for ChildComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> AnyView<()> {
            let resolved = use_context::<ProvidedCtx>().map(|c| c.0);
            *self.sink.borrow_mut() = resolved;
            any(StubLeaf)
        }
    }

    #[test]
    fn root_provided_context_resolves_in_nested_component() {
        let sink: Sink = Rc::new(RefCell::new(None));
        let root = RootComp { sink: sink.clone() };
        let root_for_build = root.clone();

        // Mirror the shells: run BOTH the state factory (`Component::init`) and
        // the initial `rebuild()` under one root `Owner`, so the context the
        // root provides in `init` is visible to the nested component whose own
        // owner is created as a child of this one during the rebuild.
        let owner = Owner::new();
        owner.with(|| {
            let mut app = new_boxed_app_with(
                move || root.init(),
                move |state: &mut ()| root_for_build.build(state),
            );
            app.rebuild();
        });

        assert_eq!(
            *sink.borrow(),
            Some(42),
            "root-provided context must resolve in the nested component through \
             the owner-wrapped new_boxed_app_with + rebuild cycle"
        );
    }

    #[test]
    fn root_context_does_not_resolve_without_ambient_owner() {
        // The negative control: the same tree with NO ambient owner. This is the
        // pre-fix behavior — `provide_context` no-ops and `use_context` returns
        // `None` — pinned so a regression that drops the `with_owner` wrap is
        // caught by the positive test above rather than passing silently.
        let sink: Sink = Rc::new(RefCell::new(None));
        let root = RootComp { sink: sink.clone() };
        let root_for_build = root.clone();

        let mut app = new_boxed_app_with(
            move || root.init(),
            move |state: &mut ()| root_for_build.build(state),
        );
        app.rebuild();

        assert_eq!(
            *sink.borrow(),
            None,
            "without an ambient owner, provide_context no-ops and use_context \
             resolves to None (the F1 bug this task closes)"
        );
    }
}
