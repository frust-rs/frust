//! [`AppTree`]: the type-erasure that lets a single non-generic native handle
//! drive any app's `State`/`app_logic`/`View`.
//!
//! Every platform shell stores its running app as a `Box<dyn AppTree>` behind an
//! opaque handle, so the FFI-exported entry points (which can't be generic) stay
//! non-generic while still driving a concrete app. This is the one seam that
//! keeps the shell runtime widget-agnostic — apps bring their own view types in
//! through the shell's app-binding macro.

use std::any::Any;

use forgekit_core::event::{EditingState, EventOutcome, ImeEvent, ImeState, InputEvent};
use forgekit_core::view::View;
use forgekit_core::{PaintOutcome, PaintScene, RenderRoot};
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
    /// Lay the tree out against a logical (density-independent) size, threading
    /// the shell-owned `TextContext` down type-erased (spec §10.3).
    fn layout(&mut self, logical: Size, text_ctx: &mut dyn Any);
    /// Paint the tree into a scene builder.
    ///
    /// Returns a [`PaintOutcome`] whose `needs_frame` is set when a widget
    /// advanced animation state during paint and wants another frame (spec's v1
    /// animation seam). The desktop shell honors it with `window.request_redraw()`;
    /// the mobile shells' continuous Choreographer/`CADisplayLink` loops already
    /// produce the next frame and may ignore it.
    fn paint(&mut self, scene: &mut dyn PaintScene) -> PaintOutcome;
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
        // app_logic is cheap by construction (spec §5); a real dirty-tracking
        // loop would skip this when state is unchanged.
        let _flags = self.root.rebuild(&mut self.logic, &mut self.state);
    }

    fn layout(&mut self, logical: Size, text_ctx: &mut dyn Any) {
        self.root.layout_with_text(logical, text_ctx);
    }

    fn paint(&mut self, scene: &mut dyn PaintScene) -> PaintOutcome {
        self.root.paint(scene)
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
}
