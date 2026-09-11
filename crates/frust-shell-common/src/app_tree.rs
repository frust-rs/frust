//! [`AppTree`]: the type-erasure that lets a single non-generic native handle
//! drive any app's `State`/`app_logic`/`View`.
//!
//! Every platform shell stores its running app as a `Box<dyn AppTree>` behind an
//! opaque handle, so the FFI-exported entry points (which can't be generic) stay
//! non-generic while still driving a concrete app. This is the one seam that
//! keeps the shell runtime widget-agnostic — apps bring their own view types in
//! through the shell's app-binding macro.

use std::any::Any;

use frust_core::accesskit;
use frust_core::anim::FrameTime;
use frust_core::event::{EditingState, EventOutcome, ImeEvent, ImeState, InputEvent};
use frust_core::insets::WindowInsets;
use frust_core::selection_toolbar::SelectionToolbarRequest;
use frust_core::view::{ChangeFlags, View};
use frust_core::widget::PlatformViewFrame;
use frust_core::{PaintOutcome, PaintScene, RenderRoot, SemanticsUpdate};
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
    /// Re-run `app_logic` and reconcile the retained tree.
    fn rebuild(&mut self);

    /// Take (and clear) the layout/paint dirtiness accumulated since the last
    /// call (delegates to [`RenderRoot::take_change_flags`]).
    ///
    /// The frame-gate seam: a shell drives the layout-skip
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
    /// A frame-gate input: a mid-drag captured widget may
    /// track/animate the pointer, so the mobile shells feed this into
    /// [`crate::FrameInputs::pointer_capture_active`] to keep producing frames
    /// while a gesture is live rather than skipping it.
    fn is_pointer_captured(&self) -> bool;

    /// Whether some widget in the tree currently holds keyboard/IME focus
    /// (delegates to [`RenderRoot::is_focus_active`]).
    ///
    /// A *level* read, used by a shell that needs the current state (an IME
    /// reconcile, a caret decision). It is deliberately **not** what the mobile
    /// frame gate consults any more — a focus session that lasts forces a frame
    /// forever — see [`AppTree::focus_ime_generation`].
    fn is_focus_active(&self) -> bool;

    /// The focus/IME session generation, bumped on every actual change of the
    /// root's focus flag or published IME surface (delegates to
    /// [`RenderRoot::focus_ime_generation`]).
    ///
    /// The frame gate's *edge* input: a shell caches the value it last saw and
    /// feeds `last != now` into
    /// [`crate::FrameInputs::focus_or_ime_changed`], so a focus/IME transition
    /// forces exactly one frame while a steady focus session (a caret blinking
    /// in an otherwise-idle field) leaves the gate free to skip or pace. The
    /// same cheap compare-a-generation shape as
    /// [`AppTree::semantics_generation`].
    ///
    /// Deliberately **not** defaulted, unlike [`AppTree::set_insets`] and the
    /// other additive methods below: any constant default (`0` included) would
    /// report "nothing ever changed" and silently strand a focus transition,
    /// against the frame gate's default-to-run rule.
    fn focus_ime_generation(&self) -> u64;

    /// Lay the tree out against a logical (density-independent) size, threading
    /// the shell-owned `TextContext` down type-erased.
    fn layout(&mut self, logical: Size, text_ctx: &mut dyn Any);
    /// Paint the tree into a scene builder at the shell-provided `frame_time`.
    ///
    /// `frame_time` is the shell's shared monotonic clock for this frame, threaded
    /// through to every animating widget as [`frust_core::widget::PaintCtx::frame_time`]
    /// (time enters from the shell, never `Instant::now()` inside the
    /// framework).
    ///
    /// Returns a [`PaintOutcome`] whose `needs_frame` is set when a widget
    /// advanced animation state during paint and wants another frame, the
    /// framework's animation seam. The desktop shell honors it with `window.request_redraw()`;
    /// the mobile shells' continuous Choreographer/`CADisplayLink` loops already
    /// produce the next frame and may ignore it.
    fn paint(&mut self, scene: &mut dyn PaintScene, frame_time: FrameTime) -> PaintOutcome;
    /// Deliver one platform input event to the retained tree.
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
    /// platform-native unit); the focused widget / `frust-text` converts them
    /// to Rust byte offsets. Returns the same [`EventOutcome`] as [`AppTree::event`].
    fn ime_apply(&mut self, state: EditingState) -> EventOutcome;

    /// The IME surface the focused widget published, for the shell to drive the
    /// platform input method. Delegates to [`RenderRoot::ime_state`]; `None` when
    /// nothing is focused or no IME surface was published.
    fn ime_state(&self) -> Option<ImeState>;

    /// Take (and clear) the text a widget asked to put on the host clipboard
    /// (delegates to [`RenderRoot::take_clipboard_write`]).
    ///
    /// The shell half of the clipboard channel: a focused editable answers a
    /// copy/cut by writing its selection into the pass's clipboard slot
    /// ([`frust_core::EventCtx::write_clipboard`]), and the shell — the only side
    /// with a host clipboard to talk to — drains it here **immediately after
    /// every [`AppTree::event`]/[`AppTree::ime_apply`]**, beside
    /// [`AppTree::ime_state`]. `None` means no widget copied, and a shell with no
    /// clipboard wired yet may simply not call this.
    ///
    /// **Destructive**, unlike the level reads around it: a clipboard write is an
    /// edge, so a caller that drains and drops the result loses that write.
    fn take_clipboard_write(&mut self) -> Option<String>;

    /// Take (and clear) whether a widget asked the shell to read the host
    /// clipboard back to it (delegates to [`RenderRoot::take_paste_request`]).
    ///
    /// The inverse direction, drained in the same place: on `true` the shell reads
    /// its host clipboard and dispatches
    /// [`InputEvent::EditCommand`]`(`[`EditCommand::Paste`](frust_core::EditCommand::Paste)`(text))`
    /// back through [`AppTree::event`] — a *new* dispatch, since the read may be
    /// asynchronous. That answer is focus-routed and therefore self-cancelling: if
    /// focus moved or was released while the read was in flight it reaches no
    /// widget and is dropped, so the shell never has to track who asked.
    ///
    /// **Destructive**, for [`AppTree::take_clipboard_write`]'s reason.
    fn take_paste_request(&mut self) -> bool;

    /// The selection-toolbar request the focused field published during the most
    /// recent [`AppTree::paint`] (delegates to
    /// [`RenderRoot::selection_toolbar`]); `None` when no field has a selection
    /// worth a toolbar.
    ///
    /// The shell half of the **platform edit-menu** route: on a host with a
    /// system edit menu (iOS `UIEditMenuInteraction`), a shell reads this beside
    /// [`AppTree::ime_state`] and presents the host menu at the request's anchor
    /// rect, converting the logical rect to the platform's own units itself. A
    /// **level**, not an edge — pair it with
    /// [`AppTree::selection_toolbar_generation`] to notice changes cheaply.
    ///
    /// A field drawing its *own* toolbar publishes this too (one code path for
    /// both routes), so a shell must gate on
    /// `frust_core::selection_toolbar::selection_toolbar_policy()`, not on the
    /// presence of a request.
    ///
    /// **Defaulted to `None`** like the getters below, so an [`AppTree`] impl
    /// that predates this channel still compiles and reads an empty one.
    fn selection_toolbar(&self) -> Option<SelectionToolbarRequest> {
        None
    }

    /// A monotonically-increasing generation bumped on every **actual** change of
    /// [`AppTree::selection_toolbar`], its clearing included (delegates to
    /// [`RenderRoot::selection_toolbar_generation`]).
    ///
    /// The [`AppTree::focus_ime_generation`] contract one channel over: a shell
    /// caches the last value it acted on and re-presents the host menu only when
    /// it moves, which is what keeps a standing selection — republished every
    /// frame it stands — from re-presenting the menu on every vsync.
    ///
    /// **Defaulted to `0`**, the same additive shape as its neighbour above.
    fn selection_toolbar_generation(&self) -> u64 {
        0
    }

    /// Store the app's active theme, threaded into every subsequent
    /// layout/paint pass (delegates to [`RenderRoot::set_theme`]).
    ///
    /// The theme is **type-erased** (`Box<dyn Any>`) so `frust-shell-common`
    /// stays free of a `frust-theme` dependency (it compiles everywhere with
    /// no unsafe/FFI — see `docs/ARCHITECTURE.md`). The concrete `Theme` is
    /// boxed by the shell that owns the appearance state; each mobile shell
    /// wires this into its own appearance-change handling. Re-boxing on a
    /// live appearance change replaces the stored theme.
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

    /// Store the shell's running count of frames the render thread has actually
    /// presented, threaded into every subsequent paint pass (delegates to
    /// [`RenderRoot::set_presented_frames`]).
    ///
    /// A shell loads the atomic its render side increments (once per presented
    /// frame) and pushes it here once per UI frame, before [`AppTree::paint`], so
    /// a widget measuring FPS reports the *presented* rate rather than its own
    /// paint cadence (which, under the render-thread split, runs faster).
    ///
    /// **Unlike [`AppTree::set_theme`]/[`AppTree::set_insets`], this dirties
    /// nothing** — [`RenderRoot::set_presented_frames`] marks no [`ChangeFlags`],
    /// so a monotonically ticking counter never forces a relayout and — the
    /// subtle one — never keeps the mobile [`frame_gate`](crate::frame_gate)'s
    /// pending-flags input perpetually true, so the menu still idles.
    /// Defaulted to a **no-op** so existing
    /// [`AppTree`] impls compile unchanged; the concrete tree overrides it.
    fn set_presented_frames(&mut self, _presented: u64) {}

    /// Store whether the shell's GPU surface is translucent (alpha-channel,
    /// "Mode B"), threaded into every subsequent paint pass (delegates to
    /// [`RenderRoot::set_surface_translucent`]).
    ///
    /// A shell pushes the surface's **resolved** translucency here — what
    /// `frust_render::SurfaceRenderer::surface_resolved_translucent` reports
    /// after an install, NOT the [`crate::SurfaceModeWatcher`] request latch
    /// (a translucency request the platform refuses must
    /// degrade to the opaque Mode A contract, or the punch presents black
    /// rectangles). Both mobile shells re-read it every frame, so a
    /// render-thread fallback downgrades within one frame. The platform-view
    /// hole-punch then clears each slot's rect on a genuinely translucent
    /// surface so an opaque app backdrop doesn't seal the hole (see
    /// [`RenderRoot::set_surface_translucent`]). Desktop leaves the default
    /// (opaque). Defaulted to a **no-op** so existing [`AppTree`] impls compile
    /// unchanged; the concrete tree overrides it — mirrors
    /// [`AppTree::set_insets`]'s default-no-op precedent.
    fn set_surface_translucent(&mut self, _translucent: bool) {}

    /// Collect the accessibility tree for the current frame,
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

    /// The [`PlatformViewFrame`]s the tree published during the most recent
    /// paint pass (delegates to
    /// [`RenderRoot::platform_view_frames`]). A shell's peek-getter path feeds
    /// this into a [`crate::platform_view::PlatformViewState`]'s
    /// [`ingest`](crate::platform_view::PlatformViewState::ingest) after each
    /// RUN frame's paint (never on a gate-`Skip`, per that method's
    /// skip-safety contract).
    ///
    /// Defaulted to an empty slice so existing [`AppTree`] impls compile
    /// unchanged; the concrete tree overrides it — mirrors
    /// [`AppTree::set_insets`]'s default-no-op precedent.
    fn platform_view_frames(&self) -> &[PlatformViewFrame] {
        &[]
    }

    /// The z-shield rects the tree reported during the most recent paint pass
    /// (delegates to [`RenderRoot::input_shields`]) — the
    /// second argument of the same
    /// [`ingest`](crate::platform_view::PlatformViewState::ingest) call
    /// [`AppTree::platform_view_frames`] feeds.
    ///
    /// Defaulted to an empty slice like the frames getter above, so an
    /// [`AppTree`] impl that predates the shield channel still compiles (and
    /// simply ships no auto-collected shields).
    fn input_shields(&self) -> &[kurbo::Rect] {
        &[]
    }

    /// Drain the slot ids whose `platform_view` widgets were torn down since the
    /// last call (delegates to
    /// [`RenderRoot::take_retired_platform_views`]).
    ///
    /// A shell calls this right after [`AppTree::rebuild`] and retires each id
    /// in its [`PlatformViewState`](crate::platform_view::PlatformViewState), so
    /// a disposed slot's native view goes away on the next frame instead of
    /// waiting out the differ's missing-streak heuristic. Draining is
    /// destructive — an id is reported exactly once.
    ///
    /// Defaulted to an empty `Vec`, mirroring the two getters above.
    fn take_retired_platform_views(&mut self) -> Vec<u64> {
        Vec::new()
    }

    /// A read-only, pre-order snapshot of the retained tree (delegates to
    /// [`RenderRoot::inspect`]) — the one thing the devtools
    /// [`DevtoolsUi`](crate::devtools::DevtoolsUi) hop needs from a mobile
    /// shell, whose handle owns its tree only as a `Box<dyn AppTree>`.
    ///
    /// Gated on the `devtools` feature: with devtools compiled out there is no
    /// caller, and the seam should not exist. Defaulted to an empty `Vec`
    /// like the getters above, so an [`AppTree`] impl outside this crate still
    /// compiles; the concrete tree overrides it.
    #[cfg(feature = "devtools")]
    fn inspect(&self) -> Vec<frust_core::InspectNode> {
        Vec::new()
    }
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
        // app_logic is cheap by construction. The returned flags are
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

    fn focus_ime_generation(&self) -> u64 {
        self.root.focus_ime_generation()
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

    fn take_clipboard_write(&mut self) -> Option<String> {
        self.root.take_clipboard_write()
    }

    fn take_paste_request(&mut self) -> bool {
        self.root.take_paste_request()
    }

    fn selection_toolbar(&self) -> Option<SelectionToolbarRequest> {
        self.root.selection_toolbar()
    }

    fn selection_toolbar_generation(&self) -> u64 {
        self.root.selection_toolbar_generation()
    }

    fn set_theme(&mut self, theme: Box<dyn Any>) {
        self.root.set_theme(theme);
    }

    fn set_insets(&mut self, insets: WindowInsets) {
        self.root.set_insets(insets);
    }

    fn set_presented_frames(&mut self, presented: u64) {
        self.root.set_presented_frames(presented);
    }

    fn set_surface_translucent(&mut self, translucent: bool) {
        self.root.set_surface_translucent(translucent);
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

    fn platform_view_frames(&self) -> &[PlatformViewFrame] {
        self.root.platform_view_frames()
    }

    fn input_shields(&self) -> &[kurbo::Rect] {
        self.root.input_shields()
    }

    fn take_retired_platform_views(&mut self) -> Vec<u64> {
        self.root.take_retired_platform_views()
    }

    #[cfg(feature = "devtools")]
    fn inspect(&self) -> Vec<frust_core::InspectNode> {
        self.root.inspect()
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
/// Called by a shell's app-binding macro (e.g. `frust::android_app!`) from its
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
    use frust_core::layout::BoxConstraints;
    use frust_core::view::{BuildCtx, ChangeFlags};
    use frust_core::widget::{LayoutCtx, PaintCtx, Widget};
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
        fn focus_ime_generation(&self) -> u64 {
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
        // The clipboard drains answer honestly instead of panicking like their
        // neighbours: "no widget asked" is a real answer a tree can give (it is
        // what a shell reads on every pass in which nothing copied), so a
        // hypothetical driver calling them on this double should see the empty
        // channel rather than a panic.
        fn take_clipboard_write(&mut self) -> Option<String> {
            None
        }
        fn take_paste_request(&mut self) -> bool {
            false
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
    fn app_tree_selection_toolbar_defaults_to_absent() {
        // Compiles (both defaults exist, so a pre-toolbar `AppTree` impl outside
        // this crate is unaffected) and reads the empty channel: no request, and
        // a generation a shell can diff from frame zero.
        let tree = MinimalTree;
        assert!(tree.selection_toolbar().is_none());
        assert_eq!(tree.selection_toolbar_generation(), 0);
    }

    #[test]
    fn app_tree_platform_view_frames_defaults_to_empty_slice() {
        // Compiles (the default impl exists) and is a benign empty read — a
        // pre-platform-views `AppTree` impl is unaffected.
        let tree = MinimalTree;
        assert!(tree.platform_view_frames().is_empty());
    }

    #[test]
    fn app_tree_shield_and_retire_channels_default_to_empty() {
        // Same additive contract for the two channels: an `AppTree` impl
        // that predates them compiles and reports nothing.
        let mut tree = MinimalTree;
        assert!(tree.input_shields().is_empty());
        assert!(tree.take_retired_platform_views().is_empty());
    }

    #[test]
    fn app_tree_set_insets_defaults_to_no_op() {
        use frust_core::insets::EdgeInsets;
        let mut tree = MinimalTree;
        // Compiles (the default impl exists) and is a benign no-op — a pre-insets
        // `AppTree` impl is unaffected.
        tree.set_insets(WindowInsets::default());
        tree.set_insets(WindowInsets::new(
            EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            EdgeInsets::ZERO,
        ));
    }

    /// A leaf that answers a clipboard verb the way a real editable does: it
    /// writes its "selection" on a copy and asks for the host clipboard on a
    /// paste command it cannot satisfy itself.
    struct ClipboardWidget;
    impl Widget for ClipboardWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::ZERO
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(
            &mut self,
            ctx: &mut frust_core::EventCtx,
            event: &InputEvent,
        ) -> frust_core::EventResult {
            match event {
                InputEvent::EditCommand(frust_core::EditCommand::Copy) => {
                    ctx.write_clipboard("selection".to_string());
                    frust_core::EventResult::Handled
                }
                InputEvent::EditCommand(frust_core::EditCommand::SelectAll) => {
                    ctx.request_paste();
                    frust_core::EventResult::Handled
                }
                _ => frust_core::EventResult::Ignored,
            }
        }
    }

    struct ClipboardView;
    impl View<NonDefaultState> for ClipboardView {
        type Element = ClipboardWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ClipboardWidget {
            ClipboardWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ClipboardWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    #[test]
    fn erased_app_delegates_the_clipboard_drains_to_the_root() {
        // The shell-facing half of the clipboard channel: a shell drains these
        // through `AppTree` and never touches `RenderRoot` directly.
        let mut app = new_boxed_app_with(
            || NonDefaultState { label: "clip" },
            |_state: &mut NonDefaultState| ClipboardView,
        );
        app.rebuild();
        app.layout(Size::new(10.0, 10.0), &mut () as &mut dyn Any);

        assert!(app.take_clipboard_write().is_none(), "nothing copied yet");
        assert!(!app.take_paste_request());

        app.event(&InputEvent::EditCommand(frust_core::EditCommand::Copy));
        assert_eq!(app.take_clipboard_write().as_deref(), Some("selection"));
        assert!(
            app.take_clipboard_write().is_none(),
            "the drain is one-shot through the erasure too"
        );

        app.event(&InputEvent::EditCommand(frust_core::EditCommand::SelectAll));
        assert!(app.take_paste_request());
        assert!(!app.take_paste_request());
    }

    #[test]
    fn app_tree_clipboard_drains_report_an_empty_channel_on_the_minimal_tree() {
        // Unlike its `unimplemented!()` neighbours these answer, because "no
        // widget asked" is a real answer a tree with no clipboard can give.
        let mut tree = MinimalTree;
        assert!(tree.take_clipboard_write().is_none());
        assert!(!tree.take_paste_request());
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

    // --- Layout-skip contract (mobile frame gate) --------------------------
    //
    // The seam the mobile shells drive: rebuild -> (layout iff needs_layout / first
    // frame / resize) -> paint. This proves the `set_theme => LAYOUT|PAINT`
    // correctness anchor: a widget that BAKES its themed value at LAYOUT time
    // (like `Text`'s glyph color) relayouts on a bare theme swap, while a
    // no-change frame skips layout yet still paints the last-baked value.

    use std::cell::Cell;

    use frust_scene::{Command, Scene, SceneBuilder};

    /// A type-erased "theme" carrying one scalar the baking widget reads at
    /// layout time — stands in for `frust_theme::Theme` (the seam needs no
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
            let color = frust_theme::ColorScheme::neutral_light().primary;
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

    // --- Root-owner regression test -----------------------------------------
    //
    // A root component's `init`/`build` run under the shell's ROOT `Owner`
    // (`Owner::with`, the way `create_handle`/`frust::run` now wrap
    // `new_boxed_app_with` + the initial rebuild). Without an ambient owner,
    // `provide_context` in the root's `init` silently no-ops and a nested
    // component's `use_context` returns `None`. This drives that exact path and
    // asserts the context resolves through the owner chain.

    use std::cell::RefCell;
    use std::rc::Rc;

    use frust_core::component::{Component, component};
    use frust_core::view::{AnyView, any};
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

    // --- WindowMetrics delivery + rebuild-cost regression test ---------------
    //
    // All three shells publish `WindowMetrics` the same way: poll a
    // `WindowMetricsPublisher` from the entry points where the window's shape
    // actually moves (surface create/resize, insets), and `provide_context` the
    // result under the reactive root owner ONLY when the poll reports a change.
    // Their own publish methods are unreachable from the host (both mobile `app`
    // modules are target-gated; the desktop one needs a live winit window), so
    // this drives that exact loop over the shared `AppTree` seam — the same
    // reason the tracked-rebuild test below lives here.
    //
    // Two things are proven together, because they trade off against each other:
    //   1. `use_context::<WindowMetrics>()` resolves inside `Component::build`
    //      (the delivery contract), and
    //   2. a static window re-provides NOTHING across a long run of frames (the
    //      cost contract). `provide_context` notifies nothing on its own — an
    //      unconditional per-frame re-provide would still cost a lock write
    //      plus an allocation every frame at the FFI boundary for no
    //      observable benefit, which is what this guards against.

    use frust_core::{Orientation, WindowMetrics};

    use crate::WindowMetricsPublisher;

    /// Records what the nested component's `build` resolved for
    /// `use_context::<WindowMetrics>()` on its most recent run.
    type MetricsSink = Rc<RefCell<Option<WindowMetrics>>>;

    /// Reads the shell-provided window shape in `build`, like a real component
    /// laying itself out around size/orientation would.
    #[derive(Clone)]
    struct MetricsComp {
        sink: MetricsSink,
    }
    impl Component for MetricsComp {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> AnyView<()> {
            *self.sink.borrow_mut() = use_context::<WindowMetrics>();
            any(StubLeaf)
        }
    }

    #[test]
    fn window_metrics_reaches_component_build_and_re_provides_only_on_change() {
        let sink: MetricsSink = Rc::new(RefCell::new(None));
        let comp = MetricsComp { sink: sink.clone() };

        // Counts actual `provide_context` calls — the app-wide invalidation a
        // re-provide represents. This is the number the cost contract is about.
        let provides = Rc::new(Cell::new(0u32));

        // The shells' shared publish body: poll, and publish only on `Some`.
        // (Each shell wraps this in `ReactiveRuntime::with_owner`/`Owner::with`;
        // here the whole run is inside one `owner.with` below, matching.)
        let mut publisher = WindowMetricsPublisher::new();
        let provides_for_publish = provides.clone();
        let publish = move |publisher: &mut WindowMetricsPublisher,
                            physical: (u32, u32),
                            scale: f64,
                            insets: WindowInsets| {
            if let Some(metrics) = publisher.poll(physical, scale, insets) {
                provide_context(metrics);
                provides_for_publish.set(provides_for_publish.get() + 1);
            }
        };

        let owner = Owner::new();
        owner.with(|| {
            // Seed before the first rebuild, exactly as each shell does (the
            // mobile shells inside `new`, desktop inside `resumed`) — a 1080x2400
            // @3x portrait phone surface, no insets reported yet.
            publish(&mut publisher, (1080, 2400), 3.0, WindowInsets::default());

            let mut app = new_boxed_app_with(|| (), move |state: &mut ()| comp.build(state));
            app.rebuild();

            // (1) Delivery: it resolved inside `Component::build`, in LOGICAL px.
            let seen = sink
                .borrow()
                .expect("WindowMetrics must reach Component::build");
            assert_eq!(seen.size, Size::new(360.0, 800.0), "logical, not physical");
            assert_eq!(seen.scale, 3.0);
            assert_eq!(seen.orientation, Orientation::Portrait);
            assert_eq!(seen.insets, WindowInsets::default());
            assert_eq!(provides.get(), 1, "the seed publishes exactly once");

            // (2) Cost: a static window over a long run of frames. Every frame
            // re-runs the publish body with the values the shell has stored
            // (nothing moved), then rebuilds. Not one re-provide may happen.
            for _ in 0..120 {
                publish(&mut publisher, (1080, 2400), 3.0, WindowInsets::default());
                app.rebuild();
            }
            assert_eq!(
                provides.get(),
                1,
                "a static window must never re-provide WindowMetrics — an \
                 unconditional per-frame re-provide would pay a lock write \
                 plus an allocation every frame at the FFI boundary for \
                 nothing observable"
            );

            // (3) A real rotation publishes once and flips the derived
            // orientation the next rebuild reads...
            publish(&mut publisher, (2400, 1080), 3.0, WindowInsets::default());
            app.rebuild();
            assert_eq!(provides.get(), 2, "a rotation is a real change");
            let seen = sink.borrow().expect("still delivered after the rotation");
            assert_eq!(seen.size, Size::new(800.0, 360.0));
            assert_eq!(seen.orientation, Orientation::Landscape);

            // ...and then goes quiet again at the new shape.
            for _ in 0..120 {
                publish(&mut publisher, (2400, 1080), 3.0, WindowInsets::default());
                app.rebuild();
            }
            assert_eq!(provides.get(), 2, "settled again after the rotation");

            // (4) The insets copy moves independently (the IME coming up at an
            // unchanged size/scale) — one more publish, then quiet.
            let ime_up = frust_core::insets::WindowInsets::new(
                frust_core::insets::EdgeInsets::new(0.0, 24.0, 0.0, 34.0),
                frust_core::insets::EdgeInsets::new(0.0, 0.0, 0.0, 340.0),
            );
            publish(&mut publisher, (2400, 1080), 3.0, ime_up);
            app.rebuild();
            publish(&mut publisher, (2400, 1080), 3.0, ime_up);
            app.rebuild();
            assert_eq!(provides.get(), 3, "an insets change publishes exactly once");
            assert_eq!(
                sink.borrow().expect("delivered").insets,
                ime_up,
                "the metrics carry a copy of the shell's already-logical insets"
            );
        });
    }

    // --- Mobile tracked-rebuild regression test ----
    //
    // The mobile shells (`frust-shell-android`/`-ios`) now wrap their
    // per-frame rebuild as `rt.with_owner(|| scope.track(|| app.rebuild()))` —
    // the exact shape this test drives over the shared `AppTree` seam (the
    // shells' own `app` modules are target-gated and never host-compiled, so this
    // is where the wrap gets host coverage). It proves the wake mechanism device
    // screens depend on: a signal read during a `scope.track`-wrapped rebuild
    // subscribes the frame scope, so a later write trips the process-wide
    // `signals_dirty` flag the mobile frame gate drains (`take_signals_dirty` →
    // `FrameInputs::signals_dirty`). The negative control is the exact bug this
    // test guards against: a bare `with_owner(|| app.rebuild())` (no `scope.track`) installs
    // the reactive Owner but NOT the Observer, so the same post-rebuild write
    // subscribes nothing and trips nothing — the gate then skips the frame that
    // would paint the loaded content until a touch forces a Run. Both halves run
    // in ONE `#[test]` because `signals_dirty` is process-global: no other
    // shell-common test touches it, so a single serial test needs no cross-test
    // lock.

    use frust_reactive::{ReactiveRuntime, TrackedScope};
    use reactive_graph::signal::RwSignal;
    use reactive_graph::traits::{Get, Set};

    #[test]
    fn scope_tracked_rebuild_trips_signals_dirty_but_bare_rebuild_does_not() {
        // A no-op waker: this test asserts on the drained `signals_dirty` flag
        // (which `TrackedScope::notify_dirty` sets on every tracked write), not
        // on wake calls, so the waker itself need do nothing.
        let rt = ReactiveRuntime::init(std::sync::Arc::new(|| {}));

        // --- Positive: the shells' new wrap subscribes the scope. ---
        let tracked_signal = rt.with_owner(|| RwSignal::new(0u32));
        let mut tracked_app: Box<dyn AppTree> = new_boxed_app_with(
            || (),
            move |_s: &mut ()| {
                // A tracked read during rebuild — under `scope.track` it
                // subscribes the scope, exactly as a real screen's
                // `controller.loading.get()` does inside a mobile rebuild.
                let _ = tracked_signal.get();
                StubLeaf
            },
        );
        let scope = TrackedScope::new();
        {
            // Disjoint borrows, mirroring the shells' `scope.track(|| app.rebuild())`.
            let s = &scope;
            let a = &mut tracked_app;
            rt.with_owner(|| s.track(|| a.rebuild()));
        }
        // Drain anything construction/rebuild left set so the assertion observes
        // only the post-rebuild write below.
        rt.take_signals_dirty();
        tracked_signal.set(1);
        assert!(
            rt.take_signals_dirty(),
            "a write to a signal read inside the scope-tracked rebuild must trip \
             signals_dirty — the wake the mobile frame gate drains"
        );

        // --- Negative control: a bare, untracked rebuild. ---
        let untracked_signal = rt.with_owner(|| RwSignal::new(0u32));
        let mut untracked_app: Box<dyn AppTree> = new_boxed_app_with(
            || (),
            move |_s: &mut ()| {
                let _ = untracked_signal.get();
                StubLeaf
            },
        );
        // The pre-fix mobile shape: `with_owner` installs the Owner but NOT the
        // reactive Observer, so the read subscribes nothing.
        rt.with_owner(|| untracked_app.rebuild());
        rt.take_signals_dirty();
        untracked_signal.set(1);
        assert!(
            !rt.take_signals_dirty(),
            "without the scope.track wrap the read subscribes nothing, so the \
             write trips no signals_dirty — the device-only stuck-on-loading stall"
        );
    }
}
