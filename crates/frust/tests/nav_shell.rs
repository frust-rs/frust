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

use frust::{
    AnyView, BackPolicy, NavigatorController, NavigatorView, PushOptions, any, handles_back,
    navigator, overlay_host, push_back_press, text,
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

/// The `app_logic` closure type [`RenderRoot::rebuild`] drives.
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
