//! Back-arbitration regression matrix over REAL `frust::navigator()`/
//! `frust::overlay_host()` compositions — pinned against the ranking code as
//! shipped (`frust`'s `back_glue` module, R44-back), not a proposed change to
//! it. A root overlay host, an outer navigator, and a further-nested (shell)
//! navigator built inside a page the outer navigator pushes is the shape a
//! nested-navigator binding introduces: three back-press registrants at
//! once, one level deeper than `back_glue`'s own two-registrant test module
//! exercises. Every case below composes only through the public facade entry
//! points an app actually calls (`navigator`/`overlay_host`, never the
//! crate-private wiring functions `back_glue`'s own tests use).
//!
//! Each test name states its own expected outcome; read a failure here as a
//! regression in ranking, the arm predicate, or the R23 reach gate — never in
//! plain navigation state, which each test's own depth assertions also pin
//! independently.
//!
//! The second half of the file builds that same shape through
//! [`shell_route`](frust::shell_route) rather than a hand-pushed page, and adds
//! the binding's own mechanics: the chain split, the keep rule (asserted as
//! widget identity, not just as stack shape), the deep-link drain on the inner
//! navigator's *first* build, the route observable over the inner navigator,
//! and the no-regression case that a chain crossing no shell still flattens
//! onto one controller.
//!
//! # Process-wide state and serialization
//!
//! `push_back_press`/`handles_back` sit behind `frust-reactive` statics that
//! are genuinely process-wide (not per-thread), so every test here takes
//! [`TEST_LOCK`] and calls [`reset`] first — mirroring `back_glue`'s own test
//! module exactly, one process (this file compiles to its own test binary)
//! away from any risk of colliding with it.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::{
    AnyView, BackPolicy, Column, NavigatorController, NavigatorView, PushOptions, Route,
    RouteObserver, RouteParams, Router, any, handles_back, navigator, overlay_host,
    push_back_press, shell_route, text,
};
use frust_core::RenderRoot;
use frust_reactive::{ReactiveRuntime, clear_can_pop_provider};

/// Serializes every test in this file — see the module doc.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Best-effort per-test reset of the state this file can reach publicly
/// (mirrors `back_glue`'s own `reset()`, minus its registrant-list clear,
/// which is crate-private and unreachable from here — each test already gets
/// fresh thread-local wiring state by construction, so nothing here relies on
/// that clear for correctness).
fn reset() {
    clear_can_pop_provider();
    frust::set_handles_back(false);
}

fn page() -> AnyView<()> {
    any(text("x"))
}

/// The build closure type [`RenderRoot::rebuild`] drives.
type AppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

// ---------------------------------------------------------------------------
// Cases 1-2: a single navigator, no host anywhere in the tree.
// ---------------------------------------------------------------------------

#[test]
fn case_1_single_navigator_depth_gt1_back_pops_it() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let c = controller.clone();
        Box::new(move |_: &mut ()| navigator(&c, page))
    };
    root.rebuild(&mut app, &mut ());

    controller.push(page);
    root.rebuild(&mut app, &mut ());
    assert_eq!(controller.depth(), 2, "pushed to depth 2");
    assert!(handles_back(), "a poppable single navigator claims back");

    push_back_press();
    root.rebuild(&mut app, &mut ());
    assert_eq!(
        controller.depth(),
        1,
        "one back press popped the single navigator"
    );
}

#[test]
fn case_2_single_navigator_depth1_back_falls_through_unclaimed() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let controller: NavigatorController<()> = NavigatorController::new();
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let c = controller.clone();
        Box::new(move |_: &mut ()| navigator(&c, page))
    };
    root.rebuild(&mut app, &mut ());

    assert_eq!(controller.depth(), 1, "starts at the root");
    assert!(
        !handles_back(),
        "depth 1, no host anywhere: the press bubbles to the platform"
    );

    // A press with nothing to claim it is still a safe no-op.
    push_back_press();
    root.rebuild(&mut app, &mut ());
    assert_eq!(controller.depth(), 1, "still at the root: nothing popped");
    assert!(!handles_back());
}

// ---------------------------------------------------------------------------
// Cases 3-4: a root overlay host wrapping ONE navigator (no shell nesting).
// ---------------------------------------------------------------------------

struct HostHarness {
    host: NavigatorController<()>,
    inner: NavigatorController<()>,
    root: RenderRoot<(), NavigatorView<()>>,
    app: AppLogic,
}

impl HostHarness {
    fn new() -> Self {
        let host: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let app: AppLogic = {
            let host_c = host.clone();
            let inner_c = inner.clone();
            Box::new(move |_: &mut ()| {
                let inner_for_host = inner_c.clone();
                overlay_host(&host_c, move || any(navigator(&inner_for_host, page)))
            })
        };
        let mut harness = Self {
            host,
            inner,
            root: RenderRoot::new(),
            app,
        };
        harness.rebuild();
        harness
    }

    fn rebuild(&mut self) {
        self.root.rebuild(&mut self.app, &mut ());
    }
}

#[test]
fn case_3_host_and_navigator_overlay_open_host_wins() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = HostHarness::new();
    h.inner.push(page);
    h.rebuild();
    h.host
        .push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
    h.rebuild();
    assert_eq!((h.host.depth(), h.inner.depth()), (2, 2));
    assert!(handles_back(), "some registrant claims the press");

    push_back_press();
    h.rebuild();
    assert_eq!(h.host.depth(), 1, "the press dismissed the host's overlay");
    assert_eq!(h.inner.depth(), 2, "and left the navigator untouched");
}

#[test]
fn case_4_host_and_navigator_no_overlay_navigator_wins() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = HostHarness::new();
    h.inner.push(page);
    h.rebuild();
    assert_eq!(h.host.depth(), 1, "the host stays empty");
    assert!(
        handles_back(),
        "the navigator's own interest answers for the app"
    );

    push_back_press();
    h.rebuild();
    assert_eq!(h.inner.depth(), 1, "the press popped the navigator");
    assert_eq!(h.host.depth(), 1, "the empty host was untouched");
}

// ---------------------------------------------------------------------------
// Cases 5-8: host + outer + a further-nested (shell) navigator — the outer
// navigator's OWN pushed page hosts the inner one, exactly as a shell route's
// children resolve onto a retained inner navigator.
// ---------------------------------------------------------------------------

struct ShellHarness {
    host: NavigatorController<()>,
    outer: NavigatorController<()>,
    inner: NavigatorController<()>,
    root: RenderRoot<(), NavigatorView<()>>,
    app: AppLogic,
}

impl ShellHarness {
    /// A host wrapping an outer navigator whose ROOT page is plain — the
    /// inner (shell) navigator does not exist until
    /// [`push_shell_page`](Self::push_shell_page) runs, matching how a real
    /// shell's inner navigator is only reachable once the outer navigator has
    /// pushed onto the page that hosts it.
    fn new() -> Self {
        let host: NavigatorController<()> = NavigatorController::new();
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let app: AppLogic = {
            let host_c = host.clone();
            let outer_c = outer.clone();
            Box::new(move |_: &mut ()| {
                let outer_for_host = outer_c.clone();
                overlay_host(&host_c, move || any(navigator(&outer_for_host, page)))
            })
        };
        let mut harness = Self {
            host,
            outer,
            inner,
            root: RenderRoot::new(),
            app,
        };
        harness.rebuild();
        harness
    }

    fn rebuild(&mut self) {
        self.root.rebuild(&mut self.app, &mut ());
    }

    /// Pushes the shell page onto `outer`: a page whose entire content is the
    /// inner navigator, wired every time this page builder runs — the shape
    /// a nested-navigator binding produces one level down.
    fn push_shell_page(&mut self) {
        let inner = self.inner.clone();
        self.outer.push(move || any(navigator(&inner, page)));
        self.rebuild();
    }
}

#[test]
fn case_5_host_outer_shell_inner_depth_gt1_inner_wins() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = ShellHarness::new();
    h.push_shell_page();
    assert_eq!(h.outer.depth(), 2, "outer hosts the shell page");
    assert_eq!(
        h.inner.depth(),
        1,
        "the shell's inner navigator starts at its root"
    );

    h.inner.push(page);
    h.rebuild();
    assert_eq!(
        (h.host.depth(), h.outer.depth(), h.inner.depth()),
        (1, 2, 2),
        "host empty, both navigators poppable"
    );
    assert!(handles_back(), "some registrant claims the press");

    push_back_press();
    h.rebuild();
    assert_eq!(h.inner.depth(), 1, "the innermost (shell) navigator popped");
    assert_eq!(
        h.outer.depth(),
        2,
        "the outer navigator's shell page is untouched"
    );
    assert_eq!(
        h.host.depth(),
        1,
        "the host, with no overlay, was never in the running"
    );
}

#[test]
fn case_6_host_outer_shell_inner_depth1_outer_wins() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = ShellHarness::new();
    h.push_shell_page();
    assert_eq!(
        (h.outer.depth(), h.inner.depth()),
        (2, 1),
        "outer poppable, inner still at its own root"
    );
    assert!(
        handles_back(),
        "the outer navigator's own interest answers for the app"
    );

    push_back_press();
    h.rebuild();
    assert_eq!(h.outer.depth(), 1, "back popped the outer's shell page");
    assert_eq!(h.host.depth(), 1, "the host was untouched");
}

#[test]
fn case_7_host_overlay_open_host_wins_both_navigators_unreachable() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = ShellHarness::new();
    h.push_shell_page();
    h.inner.push(page);
    h.rebuild();
    h.host
        .push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
    h.rebuild();
    assert_eq!(
        (h.host.depth(), h.outer.depth(), h.inner.depth()),
        (2, 2, 2),
        "every stack is individually poppable"
    );

    // R23: an overlay on the host makes every page below it un-input-routed,
    // so neither nested navigator's own interest is even reachable — ranking
    // never has to arbitrate between them.
    assert!(
        !h.outer.back_interest(),
        "outer is covered by the host's overlay"
    );
    assert!(
        !h.inner.back_interest(),
        "inner, nested a level deeper, is covered too"
    );
    assert!(handles_back(), "the host's own overlay claims the press");

    push_back_press();
    h.rebuild();
    assert_eq!(h.host.depth(), 1, "the press dismissed the host's overlay");
    assert_eq!(h.outer.depth(), 2, "outer's shell page is untouched");
    assert_eq!(h.inner.depth(), 2, "inner's stack is untouched");
}

#[test]
fn case_8_dismiss_animated_top_page_inside_the_shell_bumps_signal_pops_nothing() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = ShellHarness::new();
    h.push_shell_page();
    assert_eq!((h.outer.depth(), h.inner.depth()), (2, 1));

    let signal = Rc::new(Cell::new(0u64));
    {
        let signal = signal.clone();
        h.inner.push_with_options(
            page,
            PushOptions::transparent()
                .back(BackPolicy::DismissAnimated)
                .dismiss_signal(signal),
        );
    }
    h.rebuild();
    assert_eq!(h.inner.depth(), 2, "the dismissable page pushed");
    assert!(
        handles_back(),
        "the innermost navigator's dismissable top page claims interest"
    );

    push_back_press();
    h.rebuild();
    assert_eq!(
        h.inner.depth(),
        2,
        "DismissAnimated does not pop on back — the stack stages the exit instead"
    );
    assert_eq!(signal.get(), 1, "the dismiss signal bumped exactly once");
    assert_eq!(h.outer.depth(), 2, "outer's shell page is untouched");
    assert_eq!(h.host.depth(), 1, "the host is untouched");
}

// ---------------------------------------------------------------------------
// Shell routes: the same three-registrant shape as cases 5-8, now built by
// `shell_route` itself rather than by a hand-pushed page — so these pin the
// binding's own mechanics (chain split, keep rule, deep-link drain) against
// the arbitration the cases above already proved.
// ---------------------------------------------------------------------------

/// A leaf counting how many times it was **built** (never how often it was
/// rebuilt): the keep rule's tripwire. A retained widget is rebuilt in place;
/// one whose page was replaced is built again from scratch, taking the shell's
/// inner navigator and every page in it along.
struct BuildCounter {
    builds: Rc<Cell<u32>>,
}
struct BuildCounterWidget;
impl View<()> for BuildCounter {
    type Element = BuildCounterWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BuildCounterWidget {
        self.builds.set(self.builds.get() + 1);
        BuildCounterWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut BuildCounterWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}
impl Widget for BuildCounterWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

/// The published route stack of `controller` as plain paths — what the shell
/// assertions below compare (a route-less page reads as `-`).
fn paths(controller: &NavigatorController<()>) -> Vec<String> {
    controller
        .route_stack()
        .entries()
        .iter()
        .map(|entry| match entry {
            Some(location) => location.path.clone(),
            None => "-".to_string(),
        })
        .collect()
}

/// muxr's shape: a `/` root declared **before** a pathless shell whose children
/// are the screens inside its chrome. The shell page mounts the inner navigator
/// through the facade's own `navigator()`, so both navigators are back-wired
/// exactly as an app wires them.
struct RouterShellHarness {
    router: Router<()>,
    outer: NavigatorController<()>,
    inner: NavigatorController<()>,
    root: RenderRoot<(), NavigatorView<()>>,
    app: AppLogic,
    /// Builds of the shell page's chrome — see [`BuildCounter`].
    shell_builds: Rc<Cell<u32>>,
    /// Builds of the inner navigator's own root (placeholder) page.
    inner_root_builds: Rc<Cell<u32>>,
}

impl RouterShellHarness {
    /// Builds the router and the render root but does **not** rebuild yet, so a
    /// deep-link test can navigate before anything mounts.
    fn unmounted() -> Self {
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let shell_builds = Rc::new(Cell::new(0u32));
        let inner_root_builds = Rc::new(Cell::new(0u32));

        let shell_page = {
            let inner = inner.clone();
            let builds = shell_builds.clone();
            let root_builds = inner_root_builds.clone();
            move |_: &RouteParams| {
                let inner = inner.clone();
                let builds = builds.clone();
                let root_builds = root_builds.clone();
                any(Column(vec![
                    any(BuildCounter { builds }),
                    any(navigator(&inner, move || {
                        any(BuildCounter {
                            builds: root_builds.clone(),
                        })
                    })),
                ]))
            }
        };
        let router = Router::with_controller(
            &outer,
            vec![
                Route::new("/", |_| page()),
                shell_route(
                    &inner,
                    shell_page,
                    vec![
                        Route::new("/sessions", |_| page()),
                        Route::new("/terminal", |_| page()),
                    ],
                ),
            ],
        );
        let app: AppLogic = {
            let c = outer.clone();
            Box::new(move |_: &mut ()| navigator(&c, page))
        };
        RouterShellHarness {
            router,
            outer,
            inner,
            root: RenderRoot::new(),
            app,
            shell_builds,
            inner_root_builds,
        }
    }

    fn new() -> Self {
        let mut harness = Self::unmounted();
        harness.rebuild();
        harness
    }

    fn rebuild(&mut self) {
        self.root.rebuild(&mut self.app, &mut ());
    }
}

#[test]
fn a_shell_chain_splits_the_outer_page_from_the_inner_leaf() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = RouterShellHarness::new();
    h.router.go("/");
    h.rebuild();
    assert_eq!(paths(&h.outer), vec!["/".to_string()]);
    assert!(!h.inner.is_mounted(), "the shell is not placed yet");

    h.router.push("/sessions");
    h.rebuild();
    assert_eq!(
        paths(&h.outer),
        vec!["/".to_string(), "/sessions".to_string()],
        "the outer controller got the SHELL page, over the retained root page"
    );
    assert_eq!(
        paths(&h.inner),
        vec!["/sessions".to_string()],
        "the leaf landed on the shell's inner controller"
    );
    assert_eq!(
        h.inner.depth(),
        1,
        "the fresh inner navigator is at its root"
    );
    assert!(h.inner.is_mounted(), "the shell page mounted its navigator");
}

#[test]
fn the_shell_keep_rule_retains_the_inner_navigator_across_a_sibling_navigation() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = RouterShellHarness::new();
    h.router.push("/sessions");
    h.rebuild();
    let outer_generation = h.outer.route_generation();
    assert_eq!(h.shell_builds.get(), 1, "the shell page built once");

    h.router.go("/terminal");
    h.rebuild();
    assert_eq!(
        h.outer.route_generation(),
        outer_generation,
        "re-navigating inside the shell issued ZERO outer ops"
    );
    assert_eq!(
        h.shell_builds.get(),
        1,
        "the shell page's widget — and the inner navigator inside it — was \
         retained, not rebuilt"
    );
    assert_eq!(paths(&h.inner), vec!["/terminal".to_string()]);

    // And again, this time stacking rather than replacing inside the shell.
    h.router.push("/sessions");
    h.rebuild();
    assert_eq!(h.outer.route_generation(), outer_generation);
    assert_eq!(h.shell_builds.get(), 1);
    assert_eq!(
        paths(&h.inner),
        vec!["/terminal".to_string(), "/sessions".to_string()]
    );
}

#[test]
fn a_deep_link_into_an_unmounted_shell_lands_on_the_inner_navigators_first_build() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    // The cold-start shape: `handle_location` runs from `Component::init`,
    // before ANY navigator widget exists. The outer controller's ops are
    // drained by its own first build (already true for a top-level
    // controller) — the inner controller's are queued while it has no widget
    // at all, and must be drained by ITS first build, one nesting level down.
    let mut h = RouterShellHarness::unmounted();
    h.router.handle_location("/terminal");
    assert!(
        !h.inner.is_mounted(),
        "nothing has been built yet: the ops are pure queued intent"
    );

    h.rebuild();
    assert_eq!(
        paths(&h.outer),
        vec!["/terminal".to_string()],
        "the outer's own pre-first-frame drain placed the shell page"
    );
    assert_eq!(
        paths(&h.inner),
        vec!["/terminal".to_string()],
        "and the inner navigator's FIRST build drained the queued leaf"
    );
    assert_eq!(
        h.inner.depth(),
        1,
        "the leaf replaced the placeholder root rather than stacking over it"
    );
    assert_eq!(
        h.inner_root_builds.get(),
        1,
        "the placeholder root was built once, inside the same build pass that \
         replaced it — no frame ever laid it out, so there is no flash"
    );
}

#[test]
fn back_across_the_shell_boundary_pops_the_inner_first_then_the_outer() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    let mut h = RouterShellHarness::new();
    h.router.go("/");
    h.rebuild();
    h.router.push("/sessions");
    h.rebuild();
    h.router.push("/terminal");
    h.rebuild();
    assert_eq!((h.outer.depth(), h.inner.depth()), (2, 2));

    // Innermost-first: the inner navigator is poppable, so it takes the press.
    assert!(handles_back());
    push_back_press();
    h.rebuild();
    assert_eq!((h.outer.depth(), h.inner.depth()), (2, 1));
    assert_eq!(paths(&h.inner), vec!["/sessions".to_string()]);

    // At inner depth 1 the inner claims no interest, so the press falls
    // through to the outer navigator, which pops the shell page itself.
    assert!(handles_back(), "the outer navigator answers now");
    push_back_press();
    h.rebuild();
    assert_eq!(
        (h.outer.depth(), h.inner.depth()),
        (1, 1),
        "back at the shell's root left the shell for the page below it"
    );
    assert_eq!(paths(&h.outer), vec!["/".to_string()]);
    assert!(
        !h.inner.is_mounted(),
        "the shell page went with it, so its navigator unmounted"
    );
}

#[test]
fn case_9_shell_route_wires_the_inner_navigator_after_the_outer() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    // `Registrant::key` ranks `Role::Navigator` peers by raw wire sequence, so
    // innermost-first only holds while the inner navigator wires AFTER its
    // host. `shell_route` wires nothing itself — binding a controller to a
    // route is plain data — and the inner navigator is mounted by the shell
    // page's builder, i.e. from inside the outer navigator's own build. That
    // ordering is structural, and it is the ordering the matrix above passes
    // with; a construct that pre-wired its inner controller at table
    // construction would invert the tie-break (the `#[ignore]`d design case in
    // `back_glue`'s own tests).
    let h = RouterShellHarness::unmounted();
    assert!(
        !handles_back(),
        "building the route table wired no back registrant at all"
    );
    assert!(
        !h.inner.is_mounted(),
        "and mounted no inner navigator — that waits for the shell page"
    );

    let mut h = h;
    h.rebuild();
    h.router.push("/sessions");
    h.rebuild();
    h.router.push("/terminal");
    h.rebuild();
    assert_eq!(
        (h.outer.depth(), h.inner.depth()),
        (2, 2),
        "both navigators poppable — the case ranking has to arbitrate"
    );

    push_back_press();
    h.rebuild();
    assert_eq!(
        h.inner.depth(),
        1,
        "the structurally innermost navigator took the press"
    );
    assert_eq!(h.outer.depth(), 2, "the outer stack is untouched");
}

#[test]
fn the_route_observable_reflects_the_inner_navigation() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    // The shell page's chrome reads the INNER navigator's observer — a page
    // builder re-runs on every rebuild, so params captured at push time would
    // be stale the instant a child navigation lands.
    let outer: NavigatorController<()> = NavigatorController::new();
    let inner: NavigatorController<()> = NavigatorController::new();
    let observer = rt.with_owner(RouteObserver::new);

    let shell_page = {
        let inner = inner.clone();
        move |_: &RouteParams| {
            let inner = inner.clone();
            any(observer.observe(navigator(&inner, page)))
        }
    };
    let router = Router::with_controller(
        &outer,
        vec![
            Route::new("/", |_| page()),
            shell_route(
                &inner,
                shell_page,
                vec![
                    Route::new("/sessions", |_| page()),
                    Route::new("/terminal", |_| page()),
                ],
            ),
        ],
    );
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let c = outer.clone();
        Box::new(move |_: &mut ()| navigator(&c, page))
    };
    rt.with_owner(|| root.rebuild(&mut app, &mut ()));

    router.push("/sessions");
    rt.with_owner(|| root.rebuild(&mut app, &mut ()));
    assert_eq!(observer.path(), "/sessions");
    assert_eq!(observer.depth(), 1);

    router.push("/terminal");
    rt.with_owner(|| root.rebuild(&mut app, &mut ()));
    assert_eq!(
        observer.path(),
        "/terminal",
        "the observer tracks the inner navigation the keep rule left invisible \
         to the outer stack"
    );
    assert_eq!(observer.depth(), 2);
    assert!(!observer.is_back());
    assert_eq!(
        paths(&outer),
        vec!["-".to_string(), "/sessions".to_string()],
        "while the outer's own entry for the shell page still names the location \
         that PLACED it (its unstamped root page reads `-`)"
    );

    push_back_press();
    rt.with_owner(|| root.rebuild(&mut app, &mut ()));
    assert_eq!(observer.path(), "/sessions");
    assert!(observer.is_back(), "and the direction came out of the diff");
}

#[test]
fn a_non_shell_chain_still_flattens_onto_one_controller() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _rt = ReactiveRuntime::init(Arc::new(|| {}));
    reset();

    // The no-regression half: with no shell binding in the table, a nested
    // chain lands entirely on the router's own controller, exactly as before.
    let controller: NavigatorController<()> = NavigatorController::new();
    let router = Router::with_controller(
        &controller,
        vec![
            Route::new("/home", |_| page()),
            Route::new("/users", |_| page()).child(Route::new(":id", |_| page())),
        ],
    );
    let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
    let mut app: AppLogic = {
        let c = controller.clone();
        Box::new(move |_: &mut ()| navigator(&c, page))
    };
    root.rebuild(&mut app, &mut ());

    router.go("/users/42");
    root.rebuild(&mut app, &mut ());
    assert_eq!(
        paths(&controller),
        vec!["/users/42".to_string(), "/users/42".to_string()],
        "both chain pages on the one controller"
    );

    router.push("/home");
    root.rebuild(&mut app, &mut ());
    assert_eq!(controller.depth(), 3, "push stacked the leaf alone");
}
