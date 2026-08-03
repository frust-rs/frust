//! Router ⇄ deep-link glue: the facade is the ONLY crate
//! that sees both `frust-widgets`' [`Router`] and `frust-reactive`'s
//! deep-link source together — `frust-widgets` stays reactive-free (see
//! `Router::handle_location`'s doc) and `frust-reactive` stays router-free
//! (see `frust_reactive::deep_link`'s module docs).
//!
//! # Precedence
//!
//! A cold-start deep link always wins over an app-hardcoded
//! `initial_location` — a deliberate choice (go_router's
//! `initialLocation`-vs-link precedence is itself a judgment call upstream;
//! we choose deep-link-wins because a cold-start link represents where the OS
//! actually opened the app, which should never lose to a hardcoded default).
//!
//! # Path navigation (the `RouteNavigator` pump)
//!
//! [`RouterDeepLinks::track`] also **pumps the router's
//! [`RouteNavigator`]** — the `Send + Sync` queue a screen reaches through
//! `provide_context` (see `frust_widgets::nav::route`'s module docs for why the
//! router itself cannot ride context). This crate is where the two halves meet
//! for the same reason as the deep-link half: [`RouterDeepLinks::new`] installs
//! `ReactiveRuntime::wake` as the queue's waker, so an off-thread request
//! schedules a frame, and `track()`'s `pump()` applies it before the app's
//! `build` returns — an app already calling `track()` every build gets path
//! navigation with **no new app code**.
//!
//! # Dedupe
//!
//! [`RouterDeepLinks::track`] is meant to be called from every
//! `Component::build` (rebuild re-runs `build` far more often than a new link
//! actually arrives — see `docs/ARCHITECTURE.md`'s Component state boundary).
//! It tracks the live [`DeepLinks::latest`](frust_reactive::DeepLinks::latest)
//! signal but only calls [`Router::handle_location`] when the observed link
//! differs from the last one it consumed (a plain "consumed marker", not a
//! numeric generation counter, since [`DeepLink`] is a value type) — see this
//! module's tests for the exact contract: a rebuild re-run that observes the
//! same already-consumed link must not re-navigate.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use frust_reactive::{DeepLink, ReactiveRuntime, deep_links};
use frust_widgets::{RouteNavigator, Router};
use reactive_graph::traits::Get;

/// Normalize a raw deep-link source string into a router-ready path.
/// Platform shells push the RAW URL as delivered by the
/// OS (`fktest://item/7`, or `https://host/item/7`) into the deep-link
/// source, but the router only understands bare paths —
/// `frust_widgets::nav::path::Location::parse` splits on `/` with no
/// scheme awareness, so an unnormalized `fktest://item/7` becomes the bogus
/// path `/fktest:/item/7`. Per this module's own docs (top of file), this
/// crate is the one place that sees both the router and the deep-link
/// source, so the URL→path translation lives here — hand-rolled and
/// dependency-free (no `url` crate in the workspace), mirroring
/// `path.rs`'s own precedent.
///
/// Rules:
/// 1. No `://` in the string → already a bare path; pass through unchanged
///    (bare-path sources like navdemo's simulate-deep-link button, plus
///    defense-in-depth).
/// 2. Has `://` → split `scheme://rest`:
///    - `http`/`https` (universal/App Links): drop the host — keep from the
///      first `/` of `rest` onward (`https://host/item/7` → `/item/7`; no
///      `/` at all, e.g. `https://host`, → `/`).
///    - Any other (custom) scheme: map host+path
///      (`fktest://item/7` → `/item/7`, host `item` + `/7`; a hostless
///      `fktest:///settings` → `/settings`).
/// 3. The result always starts with `/`.
///
/// Graceful on unmappable input by construction — worst case it produces `/`
/// or an unmatched path, which `Router::handle_location` already routes to
/// the error page like any other unmatched location (see this module's
/// tests' Criterion 3); it never panics.
fn normalize_deep_link(raw: &str) -> String {
    let Some((scheme, rest)) = raw.split_once("://") else {
        // Rule 1.
        return raw.to_string();
    };

    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        // Rule 2, universal/App Links: drop the host.
        return match rest.find('/') {
            Some(idx) => rest[idx..].to_string(),
            None => "/".to_string(),
        };
    }

    // Rule 2, custom scheme: map host+path. `rest` is `host/path...` — or,
    // for a hostless `scheme:///path`, `/path...` (an empty host before the
    // first `/`).
    let (host, tail) = match rest.split_once('/') {
        Some((h, t)) => (h, Some(t)),
        None => (rest, None),
    };
    match (host.is_empty(), tail) {
        (true, Some(t)) if !t.is_empty() => format!("/{t}"),
        (true, _) => "/".to_string(),
        (false, Some(t)) if !t.is_empty() => format!("/{host}/{t}"),
        (false, _) => format!("/{host}"),
    }
}

/// A [`Router`] wired to the process-wide deep-link source (see the module
/// docs). Construct once with [`router_with_deep_links`] (or
/// [`RouterDeepLinks::new`] directly) — typically from `Component::init`,
/// storing the result in `Component::State` — then call
/// [`track`](Self::track) from every `Component::build` to keep navigating on
/// subsequent warm links.
pub struct RouterDeepLinks<State: 'static> {
    router: Router<State>,
    consumed: Rc<RefCell<Option<DeepLink>>>,
}

impl<State: 'static> RouterDeepLinks<State> {
    /// Wire `router` to the deep-link source and resolve its start location
    /// **once**: the process's cold-start link if one has already arrived
    /// (see [`DeepLinks::initial`](frust_reactive::DeepLinks::initial)),
    /// else `initial_location` (see the module docs' precedence). Drives
    /// `router` there immediately via [`Router::handle_location`].
    ///
    /// Call this exactly once per router instance (e.g. from
    /// `Component::init`) — calling it again re-navigates to the start
    /// location, discarding whatever the app already navigated to since.
    pub fn new(router: Router<State>, initial_location: &str) -> Self {
        // The router's request queue is reactive-free by construction
        // (`frust-widgets` names no `reactive_graph` symbol), so the facade
        // installs the wake callback: a request queued from a background
        // thread, or from an event handler after this frame's rebuild, must
        // still schedule a frame for `track()`'s pump to run in.
        router.route_navigator().set_waker(Arc::new(|| {
            if let Some(runtime) = ReactiveRuntime::get() {
                runtime.wake();
            }
        }));

        let links = deep_links();
        let start = links
            .initial
            .clone()
            .unwrap_or_else(|| initial_location.to_string());
        router.handle_location(&normalize_deep_link(&start));

        // The link that resolved the start location (if any) is already
        // handled — record it as consumed so the first `track()` call, which
        // will observe the same value via `latest` (a cold-start push sets
        // both `initial` and `latest` — see `frust_reactive::deep_link`'s
        // module docs), does not re-navigate to it.
        let consumed = Rc::new(RefCell::new(links.initial.map(DeepLink::new)));

        Self { router, consumed }
    }

    /// Apply everything queued on the router's
    /// [`RouteNavigator`](Self::route_navigator), then track the live
    /// deep-link signal and navigate the router to any link that hasn't been
    /// consumed yet. Call from every `Component::build` — the pump is
    /// idempotent on an empty queue, and the link half is dedup'd by a consumed
    /// marker (see the module docs), so a rebuild re-run that observes the same
    /// already-handled link is a no-op: no re-navigation, no panic even on a
    /// malformed/unmatched link (routed to the router's error page like any
    /// other unmatched location).
    ///
    /// Pumping first is what makes a queued request **zero-frame**: `build`
    /// runs before the navigator's own `rebuild`, so the controller ops this
    /// enqueues are drained in that very same reconcile pass.
    pub fn track(&self) {
        self.router.pump();

        let Some(link) = deep_links().latest.get() else {
            return;
        };
        let already_consumed = self.consumed.borrow().as_ref() == Some(&link);
        if already_consumed {
            return;
        }
        *self.consumed.borrow_mut() = Some(link.clone());
        self.router.handle_location(&normalize_deep_link(&link.url));
    }

    /// The wired router — hand its [`controller()`](Router::controller) to
    /// [`navigator`](frust_widgets::navigator), or call its navigation
    /// methods directly.
    pub fn router(&self) -> &Router<State> {
        &self.router
    }

    /// The router's [`RouteNavigator`], waker already installed. `Send + Sync`
    /// and cheap to clone, so a `Component::init` can publish it once
    /// (`provide_context(state.nav.route_navigator())`) and any screen — or
    /// background task — navigates by path without holding the router:
    ///
    /// ```ignore
    /// let nav = use_context::<RouteNavigator>().expect("provided at the root");
    /// button("Open", move |_| nav.push("/terminal?session=abc"));
    /// ```
    ///
    /// Requests land on the next [`track`](Self::track).
    pub fn route_navigator(&self) -> RouteNavigator {
        self.router.route_navigator()
    }
}

/// Convenience constructor equivalent to [`RouterDeepLinks::new`] — see its
/// docs for the precedence rule and the call-once contract.
///
/// ```no_run
/// use frust::{AnyView, Component, Route, Router, RouterDeepLinks, any, navigator, text};
///
/// #[derive(Default)]
/// struct App;
///
/// struct AppState {
///     router_links: RouterDeepLinks<AppState>,
/// }
///
/// impl Component for App {
///     type State = AppState;
///
///     fn init(&self) -> AppState {
///         let router: Router<AppState> =
///             Router::new(vec![Route::new("/", |_params| -> AnyView<AppState> {
///                 any(text("home"))
///             })]);
///         // Resolves the start location once: a cold-start deep link wins
///         // over "/" if one already arrived.
///         AppState {
///             router_links: frust::router_with_deep_links(router, "/"),
///         }
///     }
///
///     fn build(&self, state: &mut AppState) -> AnyView<AppState> {
///         // Called every rebuild: navigates on a new warm link, no-ops
///         // otherwise (see the module docs' dedupe contract).
///         state.router_links.track();
///         let controller = state.router_links.router().controller();
///         any(navigator(controller, || any(text("home"))))
///     }
/// }
///
/// frust::app!(App);
/// # fn main() {}
/// ```
pub fn router_with_deep_links<State: 'static>(
    router: Router<State>,
    initial_location: &str,
) -> RouterDeepLinks<State> {
    RouterDeepLinks::new(router, initial_location)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{AnyView, FrameTime, PaintScene, RenderRoot, View, any};
    use frust_reactive::{ReactiveRuntime, push_deep_link};
    use frust_widgets::{NavigatorView, Route, navigator};
    use kurbo::{Point, Size};
    use std::sync::Arc;

    // --- A GPU/text-free sized leaf, mirroring `router.rs`'s own test
    // fixtures, so a route's page can be distinguished by its painted size
    // without touching the `Text` widget (which panics without a threaded
    // `TextContext` — see `frust_core::widget::LayoutCtx::text_context`).

    struct SizedLeaf {
        size: Size,
    }
    struct SizedLeafWidget {
        size: Size,
    }
    impl View<()> for SizedLeaf {
        type Element = SizedLeafWidget;
        fn build(&self, _ctx: &mut frust_core::BuildCtx<'_>) -> SizedLeafWidget {
            SizedLeafWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut SizedLeafWidget,
            _ctx: &mut frust_core::BuildCtx<'_>,
        ) -> frust_core::ChangeFlags {
            element.size = self.size;
            frust_core::ChangeFlags::LAYOUT
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
        fn paint(&mut self, ctx: &mut frust_core::PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        }
    }
    fn sized(w: f64, h: f64) -> AnyView<()> {
        any(SizedLeaf {
            size: Size::new(w, h),
        })
    }

    #[derive(Default)]
    struct RecordingScene {
        rects: Vec<(Point, Size)>,
    }
    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    const HOME: Size = Size::new(10.0, 10.0);
    const PROFILE: Size = Size::new(20.0, 20.0);
    const SETTINGS: Size = Size::new(30.0, 30.0);
    const ERROR: Size = Size::new(99.0, 99.0);

    fn routes() -> Vec<Route<()>> {
        vec![
            Route::new("/", |_params| sized(HOME.width, HOME.height)),
            Route::new("/profile/:id", |_params| {
                sized(PROFILE.width, PROFILE.height)
            }),
            Route::new("/settings", |_params| {
                sized(SETTINGS.width, SETTINGS.height)
            }),
        ]
    }

    fn router_with_error_leaf() -> Router<()> {
        Router::new(routes()).error_builder(|_loc| sized(ERROR.width, ERROR.height))
    }

    /// The `app_logic` closure type [`RenderRoot::rebuild`] drives, boxed so
    /// [`Harness`] can store it as a field.
    type AppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

    /// A `RouterDeepLinks` plus the `RenderRoot`/`app` closure driving its
    /// controller's `navigator`, all built once and reused across a test's
    /// assertions — mirrors `router.rs`'s own tests: a fresh `RenderRoot` per
    /// assertion would lose the controller's already-drained op history.
    struct Harness {
        links: RouterDeepLinks<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: AppLogic,
        state: (),
    }

    impl Harness {
        fn new(router: Router<()>, initial_location: &str) -> Self {
            let links = RouterDeepLinks::new(router, initial_location);
            let controller = links.router().controller().clone();
            let app: AppLogic = Box::new(move |_: &mut ()| {
                navigator(&controller, || sized(HOME.width, HOME.height))
            });
            let mut harness = Harness {
                links,
                root: RenderRoot::new(),
                app,
                state: (),
            };
            harness.rebuild();
            harness
        }

        fn rebuild(&mut self) {
            self.root.rebuild(&mut self.app, &mut self.state);
        }

        /// Track the deep-link signal, rebuild, and return the size of the
        /// page currently painted on top of the stack.
        fn track_and_paint(&mut self) -> Size {
            self.links.track();
            self.rebuild();
            self.paint()
        }

        fn paint(&mut self) -> Size {
            self.root.layout(Size::new(100.0, 100.0));
            let mut scene = RecordingScene::default();
            self.root.paint(&mut scene, FrameTime::ZERO);
            scene
                .rects
                .last()
                .map(|(_, size)| *size)
                .unwrap_or(Size::ZERO)
        }
    }

    // All four acceptance criteria live in ONE `#[test]` function,
    // deliberately: `frust_reactive::deep_link`'s process-wide `SLOT` (in
    // particular `DeepLinks::initial`, "the first link ever pushed in this
    // process" — see its module docs) is shared by every test in this binary,
    // and this is the only test in `frust`'s suite that touches it. A
    // single function gives deterministic ordering (no cross-test race on
    // that global) without needing a crate-private test lock like
    // `frust_reactive::deep_link`'s own tests use (`WAKER_TEST_LOCK` is
    // `pub(crate)`, unreachable from here) — and this test asserts nothing
    // about waker/wake-count, the one thing that lock actually protects.
    #[test]
    fn router_deep_link_glue() {
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        // Criterion 1a: no link queued yet anywhere in this process — a fresh
        // router starts at `initial_location`, not any stale state.
        let mut cold_no_link = Harness::new(router_with_error_leaf(), "/");
        assert_eq!(cold_no_link.paint(), HOME);

        // Criterion 1b: a deep link queued before the router is constructed
        // wins over `initial_location` — this is also, deliberately, the
        // FIRST push in this test binary, so it becomes `DeepLinks::initial`
        // for the rest of this test (and the process).
        push_deep_link("/profile/42");
        let mut warm = Harness::new(router_with_error_leaf(), "/");
        assert_eq!(warm.paint(), PROFILE);

        // Criterion 2: a warm link pushed during a running session navigates
        // on the next `track()`.
        push_deep_link("/settings");
        assert_eq!(warm.track_and_paint(), SETTINGS);

        // Criterion 2 (dedupe): manually navigate elsewhere, then call
        // `track()` again with no new push — the same link must not be
        // re-handled (it would jump back to SETTINGS if it were).
        warm.links.router().go("/");
        warm.rebuild();
        assert_eq!(warm.paint(), HOME);
        assert_eq!(
            warm.track_and_paint(),
            HOME,
            "track() must not re-navigate to an already-consumed link"
        );

        // Criterion 3: a malformed/unmatched link hits the router's error
        // page — no panic.
        push_deep_link("/does/not/exist");
        assert_eq!(warm.track_and_paint(), ERROR);

        // Criterion 4 (the `RouteNavigator` pump): a request queued the way a
        // screen's event handler would — through the context-safe handle, with
        // no router in sight — is applied by the very next `track()`, with no
        // new deep link involved. Asserted inside this same function for the
        // same process-wide-slot reason as the criteria above: `track()` reads
        // `latest`, whose value is only known here.
        warm.links.route_navigator().go("/settings");
        assert_eq!(
            warm.track_and_paint(),
            SETTINGS,
            "track() must pump the RouteNavigator queue"
        );

        // ...and pumping an empty queue on every subsequent build is inert.
        warm.links.router().go("/");
        warm.rebuild();
        assert_eq!(warm.track_and_paint(), HOME);
    }

    /// The #29 acceptance criterion: a [`RouteNavigator`] round-trips through
    /// `provide_context`/`use_context` (which require `Send + Sync + 'static`)
    /// and the recovered clone drives the *same* queue. Deliberately does not
    /// build a [`RouterDeepLinks`], so it never touches the process-wide
    /// deep-link slot the test above owns.
    #[test]
    fn route_navigator_round_trips_through_context() {
        use frust_reactive::{provide_context, use_context};

        let rt = ReactiveRuntime::init(Arc::new(|| {}));
        let router = router_with_error_leaf();
        let nav = router.route_navigator();

        let recovered = rt.with_owner(|| {
            provide_context(nav.clone());
            use_context::<RouteNavigator>()
        });
        let recovered = recovered.expect("a RouteNavigator must survive provide_context");

        // Same queue, not a detached copy: what the recovered handle asks for is
        // what the router pumps.
        recovered.push("/settings");
        router.pump();
        assert_eq!(
            nav.location().map(|loc| loc.path),
            Some("/settings".to_string()),
            "the context-recovered handle drove the original router"
        );
    }

    // --- `normalize_deep_link`: one rule
    // branch per test, mirroring `path.rs`'s test-heavy style for its own
    // hand-rolled parser.

    #[test]
    fn normalize_bare_path_passes_through() {
        // Rule 1: no `://` at all — pass through unchanged, including
        // already-normalized paths and query strings.
        assert_eq!(normalize_deep_link("/item/7"), "/item/7");
        assert_eq!(normalize_deep_link("/"), "/");
        assert_eq!(normalize_deep_link("/search?q=asdf"), "/search?q=asdf");
    }

    #[test]
    fn normalize_custom_scheme_host_and_path() {
        // Rule 2, custom scheme: host + path segments both map into the
        // result path.
        assert_eq!(normalize_deep_link("fktest://item/7"), "/item/7");
        assert_eq!(
            normalize_deep_link("fktest://item/7/nested"),
            "/item/7/nested"
        );
    }

    #[test]
    fn normalize_custom_scheme_host_only() {
        // Custom scheme with a host and no further path segments.
        assert_eq!(normalize_deep_link("fktest://settings"), "/settings");
    }

    #[test]
    fn normalize_custom_scheme_hostless() {
        // Rule 2, custom scheme, hostless (`scheme:///path`): the empty host
        // before the triple slash contributes nothing.
        assert_eq!(normalize_deep_link("fktest:///settings"), "/settings");
    }

    #[test]
    fn normalize_https_universal_link_drops_host() {
        // Rule 2, http(s): the host is dropped entirely, keeping only the
        // path onward.
        assert_eq!(normalize_deep_link("https://host/item/7"), "/item/7");
        assert_eq!(normalize_deep_link("http://host/item/7"), "/item/7");
        // Scheme match is case-insensitive.
        assert_eq!(normalize_deep_link("HTTPS://host/item/7"), "/item/7");
    }

    #[test]
    fn normalize_trailing_and_empty_edge_cases() {
        // https with a trailing slash and no further segments -> root.
        assert_eq!(normalize_deep_link("https://host/"), "/");
        // https with no path at all -> root.
        assert_eq!(normalize_deep_link("https://host"), "/");
        // Custom scheme with a trailing slash after the host -> host only.
        assert_eq!(normalize_deep_link("fktest://item/"), "/item");
        // Custom scheme with nothing after the scheme delimiter at all ->
        // root, gracefully (no panic on unmappable input).
        assert_eq!(normalize_deep_link("fktest://"), "/");
        // Empty string has no `://` -> Rule 1 passthrough, unchanged.
        assert_eq!(normalize_deep_link(""), "");
    }
}
