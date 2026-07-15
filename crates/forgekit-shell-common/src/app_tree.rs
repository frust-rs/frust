//! [`AppTree`]: the type-erasure that lets a single non-generic native handle
//! drive any app's `State`/`app_logic`/`View`.
//!
//! Every platform shell stores its running app as a `Box<dyn AppTree>` behind an
//! opaque handle, so the FFI-exported entry points (which can't be generic) stay
//! non-generic while still driving a concrete app. This is the one seam that
//! keeps the shell runtime widget-agnostic — apps bring their own view types in
//! through the shell's app-binding macro.

use std::any::Any;

use forgekit_core::event::{EventOutcome, InputEvent};
use forgekit_core::view::View;
use forgekit_core::{PaintScene, RenderRoot};
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
    fn paint(&mut self, scene: &mut dyn PaintScene);
    /// Deliver one platform input event to the retained tree (spec §9).
    ///
    /// Delegates to [`RenderRoot::event`], threading the erased `State` the same
    /// way [`AppTree::rebuild`] does. The returned [`EventOutcome`] carries
    /// `needs_redraw`, which the shell honours by scheduling a frame: the desktop
    /// shell calls `window.request_redraw()`, while the mobile shells' continuous
    /// Choreographer/`CADisplayLink` loops already produce the next frame. The
    /// event pass itself never rebuilds or repaints (see [`RenderRoot::event`]).
    fn event(&mut self, event: &InputEvent) -> EventOutcome;
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

    fn paint(&mut self, scene: &mut dyn PaintScene) {
        self.root.paint(scene);
    }

    fn event(&mut self, event: &InputEvent) -> EventOutcome {
        self.root.event(&mut self.state, event)
    }
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
    Box::new(ErasedApp {
        state,
        logic,
        root: RenderRoot::new(),
    })
}
