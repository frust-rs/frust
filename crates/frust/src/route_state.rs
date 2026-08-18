//! The facade's reactive route-state observable: the signal face over
//! `frust_widgets`' signal-free [`RouteStack`]/[`NavChange`], riding
//! `provide_context` exactly like [`RouteNavigator`] does — the difference is
//! that `RouteNavigator` carries *intent* (queued, undrained requests) while
//! [`RouteObserver`] carries *fact* (the last-published stack), the same split
//! `route_state`'s (`frust_widgets::nav::route_state`) own module docs draw.
//!
//! This is the one place the facade sees both `frust_widgets`'
//! [`NavigatorView::on_route_change`] and `frust-reactive`'s `RwSignal`
//! together — `frust-widgets` stays reactive-free (see
//! `docs/WIDGETS_ARCHITECTURE.md`'s Reactive-Free Design section), so the
//! bridge lives here, wired from [`RouterDeepLinks`](crate::RouterDeepLinks)
//! alongside its existing deep-link waker.

use frust_reactive::RwSignal;
use frust_widgets::{Location, NavChange, NavigatorView, RouteStack};
use reactive_graph::traits::{Get, Set};

/// Reactive face of one navigator's route stack: `Copy + Send + Sync`
/// (five `RwSignal`s and nothing else), so it rides `provide_context` — the
/// navigator's own [`NavigatorController`](frust_widgets::NavigatorController)/
/// [`Router`](frust_widgets::Router) never can (see
/// `frust_widgets::nav::route`'s module docs for why).
#[derive(Clone, Copy)]
pub struct RouteObserver {
    current: RwSignal<Option<Location>>,
    stack: RwSignal<Vec<Option<Location>>>,
    depth: RwSignal<usize>,
    change: RwSignal<NavChange>,
    generation: RwSignal<u64>,
}

/// The seam's whole point, pinned at compile time: if `RouteObserver` ever
/// stops being `Send + Sync + 'static` it can no longer ride
/// `provide_context`, and this becomes a compile error rather than a review
/// finding — mirrors `RouteNavigator`'s own assertion
/// (`frust_widgets::nav::route`).
const _: fn() = || {
    fn assert<T: Send + Sync + 'static>() {}
    assert::<RouteObserver>();
};

impl RouteObserver {
    /// A fresh observer at rest (`NavChange::Initial`, depth 0, generation 0,
    /// no current route) — construct once, typically in `Component::init`,
    /// and keep the result in `State`.
    pub fn new() -> Self {
        Self {
            current: RwSignal::new(None),
            stack: RwSignal::new(Vec::new()),
            depth: RwSignal::new(0),
            change: RwSignal::new(NavChange::default()),
            generation: RwSignal::new(0),
        }
    }

    /// Attach: installs [`NavigatorView::on_route_change`], writing these
    /// signals from inside the navigator's own rebuild whenever the published
    /// [`RouteStack`] actually changes. Chainable and cheap to call again
    /// (`RouteObserver` is `Copy`), so a second navigator elsewhere in the app
    /// gets its own observer the same way:
    /// `state.routes.observe(navigator(&controller, initial))`.
    ///
    /// # Wake correctness
    ///
    /// The write happens inside the navigator's rebuild, *after* `apply_ops`:
    /// a navigation requested in frame N is published in N, read by the app's
    /// `build` in N+1, and the write itself schedules N+1 — the same
    /// tracked-write-wakes-the-shell bridge every other signal in the
    /// framework rides (`docs/CODE_STANDARDS.md`'s tracked-scope rule), not a
    /// manual `ReactiveRuntime::wake()` call. One frame behind is possible;
    /// *silently* behind is not (a page transition runs ~300ms; 16ms of bar
    /// lag is below the perceptual floor).
    pub fn observe<State: 'static>(self, view: NavigatorView<State>) -> NavigatorView<State> {
        view.on_route_change(move |stack: &RouteStack| {
            self.current.set(stack.current_route().cloned());
            self.stack.set(stack.entries().to_vec());
            self.depth.set(stack.depth());
            self.change.set(stack.change());
            self.generation.set(stack.generation());
        })
    }

    /// The topmost page that **has** a route — skips a route-less
    /// (overlay/dialog) page on top, the read chrome (an app bar title, e.g.)
    /// wants. Tracked read; see [`RouteStack::current_route`]'s doc for the
    /// full contract.
    pub fn current(&self) -> Option<Location> {
        self.current.get()
    }

    /// [`current`](Self::current)'s path, or an empty string before the first
    /// publish (or if no page in the stack carries a route at all).
    pub fn path(&self) -> String {
        self.current().map(|loc| loc.path).unwrap_or_default()
    }

    /// A query parameter off [`current`](Self::current)'s location. `None` if
    /// there is no current routed page, or the key is absent.
    pub fn param(&self, key: &str) -> Option<String> {
        self.current().and_then(|loc| loc.query.get(key).cloned())
    }

    /// The full stack, bottom→top; `None` at a route-less (overlay/dialog)
    /// page — the tracked-read mirror of [`RouteStack::entries`].
    pub fn stack(&self) -> Vec<Option<Location>> {
        self.stack.get()
    }

    /// The stack depth as of the last publish.
    pub fn depth(&self) -> usize {
        self.depth.get()
    }

    /// The diff label for the most recent publish — see [`NavChange`].
    pub fn change(&self) -> NavChange {
        self.change.get()
    }

    /// Whether the most recent change was a [`NavChange::Pop`] — the
    /// direction chrome resolves a title-transition from, e.g.
    /// `.title_direction(if routes.is_back() { Back } else { Forward })`.
    pub fn is_back(&self) -> bool {
        self.change() == NavChange::Pop
    }
}

impl Default for RouteObserver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{
        AnyView, BuildCtx, ChangeFlags, FrameTime, PaintScene, RenderRoot, View, any,
    };
    use frust_reactive::ReactiveRuntime;
    use frust_widgets::{NavigatorController, NavigatorView, PushOptions, navigator};
    use kurbo::Size;
    use std::sync::Arc;

    struct SizedLeaf {
        size: Size,
    }
    struct SizedLeafWidget {
        size: Size,
    }
    impl View<()> for SizedLeaf {
        type Element = SizedLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SizedLeafWidget {
            SizedLeafWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut SizedLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.size = self.size;
            ChangeFlags::NONE
        }
    }
    impl frust_core::Widget for SizedLeafWidget {
        fn layout(
            &mut self,
            _ctx: &mut frust_core::LayoutCtx,
            bc: &frust_core::BoxConstraints,
        ) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _ctx: &mut frust_core::PaintCtx, _scene: &mut dyn PaintScene) {}
    }
    fn sized(w: f64, h: f64) -> AnyView<()> {
        any(SizedLeaf {
            size: Size::new(w, h),
        })
    }

    // A minimal `PaintScene` that just needs to exist for `paint` to run —
    // mirrors `router_glue.rs`'s own test fixture (this crate has no
    // `test-support` feature dependency on `frust-widgets`).
    #[derive(Default)]
    struct RecordingScene;
    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _origin: kurbo::Point, _size: Size, _color: peniko::Color) {}
        fn draw_text(&mut self, _origin: kurbo::Point, _text: &str) {}
    }

    /// `RouteObserver` compile-asserted `Send + Sync` (the `const _` above),
    /// round-trips `provide_context`, and a navigation applied in rebuild N is
    /// readable in build N+1.
    #[test]
    fn route_observer_rides_context_and_updates_across_a_navigation() {
        use frust_reactive::{provide_context, use_context};

        let rt = ReactiveRuntime::init(Arc::new(|| {}));
        let controller: NavigatorController<()> = NavigatorController::new();
        let observer = rt.with_owner(RouteObserver::new);

        let recovered = rt.with_owner(|| {
            provide_context(observer);
            use_context::<RouteObserver>()
        });
        let recovered = recovered.expect("a RouteObserver must survive provide_context");

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| recovered.observe(navigator(&ctrl, || sized(10.0, 10.0)))
        };
        let mut state = ();

        // Build N: no navigation yet — at rest.
        rt.with_owner(|| root.rebuild(&mut app, &mut state));
        assert_eq!(recovered.depth(), 1);
        assert_eq!(recovered.change(), NavChange::Initial);

        // A navigation applied in rebuild N ...
        controller.push_with_options(
            || sized(20.0, 20.0),
            PushOptions::opaque().route(Location::parse("/detail")),
        );
        rt.with_owner(|| root.rebuild(&mut app, &mut state));
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene;
        root.paint(&mut scene, FrameTime::ZERO);

        // ... is readable via the SAME `RouteObserver` handle in build N+1
        // (rebuild already ran build N+1's logic above; the write happened
        // inside it, which is the point — no extra frame was needed for the
        // signal write itself to land).
        assert_eq!(recovered.depth(), 2);
        assert_eq!(recovered.change(), NavChange::Push);
        assert_eq!(recovered.path(), "/detail");
        assert!(!recovered.is_back());
    }
}
