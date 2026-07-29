//! The declarative router: a go_router-subset layer over the imperative
//! [`navigator`](super::navigator).
//!
//! # Shape
//!
//! A [`Route`] pairs a path pattern (`/users/:id`) with a page builder and
//! optional `name`, per-route `redirect`, and nested `children` (whose patterns
//! compose parent + child, go_router style). A [`Router`] owns a route table, a
//! [`NavigatorController`] it drives, an `error_builder` fallback, an optional
//! top-level `redirect`, and a `redirect_limit` (go_router's default of 5).
//!
//! # Resolution
//!
//! [`Router::resolve`] turns a location string into a [`Resolution`]:
//! percent-parsed via [`Location`], matched against the route tree (capturing
//! `:param`s and composing nested chains), with **per-route and top-level
//! redirects applied under a loop guard** — exceeding `redirect_limit` falls back
//! to the error route, as does an unmatched location. `resolve` is pure (no
//! controller side effects), so the matcher/redirect logic is unit-testable
//! without a navigator.
//!
//! # Navigation API
//!
//! [`go`](Router::go) resets the stack to the matched chain (replace semantics —
//! the current top's state is dropped); [`push`](Router::push) stacks the matched
//! leaf (the page below is retained); [`pop`](Router::pop) pops one page.
//! [`go_named`](Router::go_named)/[`push_named`](Router::push_named) resolve a
//! named route's params into a path first. Every one drives the owned
//! [`NavigatorController`] — no reactive types here; the signal glue that feeds
//! [`handle_location`](Router::handle_location) deep links lives in the facade,
//! keeping `frust-widgets` reactive-free.
//!
//! The router is plain data + logic an app keeps in its `Component::State`
//! alongside the controller — no global registry.
//!
//! # Deferred (documented seams)
//!
//! * **Full arbitrary-depth stack reset for `go`.** The [`NavigatorController`]
//!   exposes `push`/`pop`/`replace` only (no "pop to root"/"clear"). `go` therefore
//!   *replaces the current top* and pushes the chain's remaining pages, dropping
//!   the replaced page's state — correct for the common flat case and for the
//!   depth-1 root case. Resetting a deeper stack to a shorter chain (dropping
//!   pages *below* the top) needs a controller reset op; recorded for a later
//!   navigator revision rather than reaching across into `navigator.rs`.
//! * **ShellRoute-style wrappers.** Nested routes compose paths and build one page
//!   per chain route today; a parent route that *wraps* its child's page in shared
//!   chrome is left for a future addition.

use std::collections::HashSet;
use std::rc::Rc;

use frust_core::{AnyView, any};

use super::navigator::NavigatorController;
use super::path::{Location, PathPattern, RouteParams, encode_segment};

/// A page builder for a route: maps the captured [`RouteParams`] to the page's
/// view. Re-run each time the page rebuilds (via the closure the router hands the
/// [`NavigatorController`]), so a page reconciles against live app state.
pub type RouteBuilder<State> = Rc<dyn Fn(&RouteParams) -> AnyView<State>>;

/// A redirect: given the resolved [`Location`], optionally return a different
/// location string to redirect to (`None` = no redirect). Used both per-route and
/// top-level; the [`Router`]'s loop guard bounds a redirect chain.
pub type Redirect = Rc<dyn Fn(&Location) -> Option<String>>;

/// A fallback page builder for an unmatched (or redirect-looping) location.
pub type ErrorBuilder<State> = Rc<dyn Fn(&Location) -> AnyView<State>>;

/// go_router's default redirect limit — the number of redirects a single
/// resolution may follow before falling back to the error route.
pub const DEFAULT_REDIRECT_LIMIT: usize = 5;

/// One entry in a [`Router`]'s route table: a path pattern, a page builder, and
/// optional `name`, per-route `redirect`, and nested `children`. Built
/// fluently — [`Route::new`] then `.name`/`.redirect`/`.child`/`.children`.
///
/// Nested children's patterns compose with this route's (go_router semantics): a
/// route `/users` with a child `:id` matches `/users/42`, and the matched chain
/// is `[/users, /users/:id]`.
pub struct Route<State: 'static> {
    path: String,
    name: Option<String>,
    builder: RouteBuilder<State>,
    redirect: Option<Redirect>,
    children: Vec<Route<State>>,
}

impl<State: 'static> Route<State> {
    /// A route matching `path` (a pattern like `/users/:id`, or a relative child
    /// path like `:id`), rendered by `builder`.
    pub fn new(
        path: impl Into<String>,
        builder: impl Fn(&RouteParams) -> AnyView<State> + 'static,
    ) -> Self {
        Route {
            path: path.into(),
            name: None,
            builder: Rc::new(builder),
            redirect: None,
            children: Vec::new(),
        }
    }

    /// Give this route a `name` for [`go_named`](Router::go_named)/
    /// [`push_named`](Router::push_named) resolution.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach a per-route redirect, consulted (against the resolved location)
    /// whenever this route is part of a matched chain.
    pub fn redirect(mut self, redirect: impl Fn(&Location) -> Option<String> + 'static) -> Self {
        self.redirect = Some(Rc::new(redirect));
        self
    }

    /// Add one nested child route (its path composes with this route's).
    pub fn child(mut self, child: Route<State>) -> Self {
        self.children.push(child);
        self
    }

    /// Replace this route's child list.
    pub fn children(mut self, children: Vec<Route<State>>) -> Self {
        self.children = children;
        self
    }
}

/// One resolved page in a [`Resolution::Matched`] chain: the route's builder plus
/// the params captured for it. [`build`](ResolvedPage::build) produces the page
/// view; the router wraps it in a `Fn() -> AnyView` for the controller.
pub struct ResolvedPage<State: 'static> {
    builder: RouteBuilder<State>,
    params: RouteParams,
}

impl<State: 'static> ResolvedPage<State> {
    /// Build the page view against its captured params.
    pub fn build(&self) -> AnyView<State> {
        (self.builder)(&self.params)
    }

    /// Turn this page into the `Fn() -> AnyView` closure a
    /// [`NavigatorController`] op takes (re-run each rebuild).
    fn into_page_builder(self) -> impl Fn() -> AnyView<State> + 'static {
        let ResolvedPage { builder, params } = self;
        move || (builder)(&params)
    }
}

/// The outcome of [`Router::resolve`]: either a matched chain of pages (with the
/// final resolved location and merged params) or an error fallback.
pub enum Resolution<State: 'static> {
    /// The location matched a route chain. `pages` is the chain root→leaf; for a
    /// flat route it holds a single page. `params` is the merged capture map,
    /// `location` the final (post-redirect) location.
    Matched {
        location: Location,
        params: RouteParams,
        pages: Vec<ResolvedPage<State>>,
    },
    /// No route matched (or a redirect loop exceeded the limit); the caller falls
    /// back to the router's `error_builder`.
    Error { location: Location },
}

impl<State: 'static> Resolution<State> {
    /// Whether this resolution matched a route (vs. the error fallback).
    pub fn is_matched(&self) -> bool {
        matches!(self, Resolution::Matched { .. })
    }

    /// The merged captured params, or an empty map for an error resolution.
    pub fn params(&self) -> RouteParams {
        match self {
            Resolution::Matched { params, .. } => params.clone(),
            Resolution::Error { .. } => RouteParams::new(),
        }
    }

    /// The number of pages in a matched chain (0 for an error resolution).
    pub fn page_count(&self) -> usize {
        match self {
            Resolution::Matched { pages, .. } => pages.len(),
            Resolution::Error { .. } => 0,
        }
    }
}

/// A declarative router over a [`navigator`](super::navigator). See the [module
/// docs](self).
pub struct Router<State: 'static> {
    routes: Vec<Route<State>>,
    controller: NavigatorController<State>,
    redirect: Option<Redirect>,
    error_builder: ErrorBuilder<State>,
    redirect_limit: usize,
}

impl<State: 'static> Router<State> {
    /// A router over `routes` with a fresh [`NavigatorController`], the default
    /// error page, and the default redirect limit ([`DEFAULT_REDIRECT_LIMIT`]).
    pub fn new(routes: Vec<Route<State>>) -> Self {
        Self::with_controller(&NavigatorController::new(), routes)
    }

    /// A router driving `controller` (so an app can hand the same controller to
    /// [`navigator`](super::navigator::navigator)).
    pub fn with_controller(
        controller: &NavigatorController<State>,
        routes: Vec<Route<State>>,
    ) -> Self {
        Router {
            routes,
            controller: controller.clone(),
            redirect: None,
            error_builder: Rc::new(default_error_page),
            redirect_limit: DEFAULT_REDIRECT_LIMIT,
        }
    }

    /// Set a top-level redirect, consulted before matching on every resolution.
    pub fn redirect(mut self, redirect: impl Fn(&Location) -> Option<String> + 'static) -> Self {
        self.redirect = Some(Rc::new(redirect));
        self
    }

    /// Replace the error-page builder (default: a simple themed "not found" page).
    pub fn error_builder(
        mut self,
        error_builder: impl Fn(&Location) -> AnyView<State> + 'static,
    ) -> Self {
        self.error_builder = Rc::new(error_builder);
        self
    }

    /// Override the redirect loop limit (default [`DEFAULT_REDIRECT_LIMIT`]).
    pub fn redirect_limit(mut self, limit: usize) -> Self {
        self.redirect_limit = limit;
        self
    }

    /// The controller this router drives — hand it to
    /// [`navigator`](super::navigator::navigator) so the app's page stack and the
    /// router share one op queue.
    pub fn controller(&self) -> &NavigatorController<State> {
        &self.controller
    }

    /// Resolve a location string into a [`Resolution`] (pure; no controller side
    /// effects). Applies top-level and per-route redirects under the loop guard;
    /// an unmatched location or an over-limit redirect chain yields
    /// [`Resolution::Error`].
    pub fn resolve(&self, raw: &str) -> Resolution<State> {
        let mut loc = Location::parse(raw);
        let mut redirects = 0usize;

        loop {
            // 1. Top-level redirect (before matching).
            if let Some(next) = self.fire_redirect(self.redirect.as_ref(), &loc) {
                redirects += 1;
                if redirects > self.redirect_limit {
                    return Resolution::Error { location: loc };
                }
                loc = next;
                continue;
            }

            // 2. Match against the route tree.
            let Some((chain, params)) = self.match_chain(&loc) else {
                return Resolution::Error { location: loc };
            };

            // 3. Per-route redirects along the matched chain (first one wins).
            let mut redirected = None;
            for route in &chain {
                if let Some(next) = self.fire_redirect(route.redirect.as_ref(), &loc) {
                    redirected = Some(next);
                    break;
                }
            }
            if let Some(next) = redirected {
                redirects += 1;
                if redirects > self.redirect_limit {
                    return Resolution::Error { location: loc };
                }
                loc = next;
                continue;
            }

            // 4. Matched, no redirect: build the page chain.
            let pages = chain
                .iter()
                .map(|route| ResolvedPage {
                    builder: route.builder.clone(),
                    params: params.clone(),
                })
                .collect();
            return Resolution::Matched {
                location: loc,
                params,
                pages,
            };
        }
    }

    /// Fire a redirect fn against `loc`, returning the parsed new location only if
    /// it actually differs (a redirect to the same location is treated as "no
    /// redirect" — it makes no progress and must not be counted toward the limit).
    fn fire_redirect(&self, redirect: Option<&Redirect>, loc: &Location) -> Option<Location> {
        let redirect = redirect?;
        let next = redirect(loc)?;
        let next_loc = Location::parse(&next);
        if next_loc.location_string() == loc.location_string() {
            None
        } else {
            Some(next_loc)
        }
    }

    /// Match `loc`'s segments against the route tree, returning the matched chain
    /// (root→leaf) and merged params, or `None` if nothing matches.
    fn match_chain<'a>(&'a self, loc: &Location) -> Option<(Vec<&'a Route<State>>, RouteParams)> {
        match_routes(&self.routes, &loc.segments, RouteParams::new(), Vec::new())
    }

    // --- Navigation API (drives the controller) ---

    /// Reset the stack to the matched chain (replace semantics — the current top's
    /// state is dropped). See the [module docs](self)'s deferred note on
    /// arbitrary-depth reset.
    pub fn go(&self, location: &str) {
        match self.resolve(location) {
            Resolution::Matched { pages, .. } => {
                let mut pages = pages.into_iter();
                if let Some(first) = pages.next() {
                    self.controller.replace(first.into_page_builder());
                }
                for page in pages {
                    self.controller.push(page.into_page_builder());
                }
            }
            Resolution::Error { location } => {
                self.controller.replace(self.error_page_builder(location));
            }
        }
    }

    /// Push the matched leaf page onto the stack (the page below is retained).
    pub fn push(&self, location: &str) {
        match self.resolve(location) {
            Resolution::Matched { pages, .. } => {
                if let Some(leaf) = pages.into_iter().next_back() {
                    self.controller.push(leaf.into_page_builder());
                }
            }
            Resolution::Error { location } => {
                self.controller.push(self.error_page_builder(location));
            }
        }
    }

    /// Pop the top page (a no-op on the root page — see
    /// [`NavigatorController::pop`]).
    pub fn pop(&self) {
        self.controller.pop();
    }

    /// The single entry point a deep link resolves through — same reset semantics
    /// as [`go`](Self::go). The facade tracks the deep-link signal and
    /// calls this; this crate stays reactive-free.
    pub fn handle_location(&self, location: &str) {
        self.go(location);
    }

    /// Resolve `name` + `params` into a path, then [`go`](Self::go) to it. An
    /// unknown name routes to the error page.
    pub fn go_named(&self, name: &str, params: &RouteParams) {
        match self.path_for_name(name, params) {
            Some(path) => self.go(&path),
            None => self
                .controller
                .replace(self.error_page_builder(named_error_location(name))),
        }
    }

    /// Resolve `name` + `params` into a path, then [`push`](Self::push) it. An
    /// unknown name routes to the error page.
    pub fn push_named(&self, name: &str, params: &RouteParams) {
        match self.path_for_name(name, params) {
            Some(path) => self.push(&path),
            None => self
                .controller
                .push(self.error_page_builder(named_error_location(name))),
        }
    }

    /// Build the full path for a named route, substituting `:param`s from `params`
    /// (composing nested parent + child patterns). Params not consumed by a path
    /// segment become query parameters (go_router parity). `None` if no route has
    /// the name.
    pub fn path_for_name(&self, name: &str, params: &RouteParams) -> Option<String> {
        let pattern = find_named_pattern(&self.routes, name, "")?;
        Some(substitute_pattern(&pattern, params))
    }

    /// Wrap the error builder into a `Fn() -> AnyView` closure for the controller.
    fn error_page_builder(&self, location: Location) -> impl Fn() -> AnyView<State> + 'static {
        let error_builder = self.error_builder.clone();
        move || (error_builder)(&location)
    }
}

/// Recursively match `routes` against the remaining location `segments`,
/// accumulating params and the visited route chain. A route matches as a *prefix*;
/// if it consumes all remaining segments it is the chain leaf, otherwise the match
/// recurses into its children with the remainder.
fn match_routes<'a, State: 'static>(
    routes: &'a [Route<State>],
    segments: &[String],
    acc_params: RouteParams,
    chain: Vec<&'a Route<State>>,
) -> Option<(Vec<&'a Route<State>>, RouteParams)> {
    for route in routes {
        let pattern = PathPattern::parse(&route.path);
        let Some((consumed, params)) = pattern.match_prefix(segments) else {
            continue;
        };

        let mut next_params = acc_params.clone();
        next_params.extend(params);

        let mut next_chain = chain.clone();
        next_chain.push(route);

        let remaining = &segments[consumed..];
        if remaining.is_empty() {
            return Some((next_chain, next_params));
        }
        // More segments left: only a child match can consume them.
        if let Some(found) = match_routes(&route.children, remaining, next_params, next_chain) {
            return Some(found);
        }
        // This route matched a prefix but no child covered the rest — keep trying
        // sibling routes.
    }
    None
}

/// Walk the route tree for a route named `name`, composing the full path pattern
/// (parent `prefix` + each route's own path). `None` if not found.
fn find_named_pattern<State: 'static>(
    routes: &[Route<State>],
    name: &str,
    prefix: &str,
) -> Option<String> {
    for route in routes {
        let full = join_paths(prefix, &route.path);
        if route.name.as_deref() == Some(name) {
            return Some(full);
        }
        if let Some(found) = find_named_pattern(&route.children, name, &full) {
            return Some(found);
        }
    }
    None
}

/// Join a parent path prefix with a child path, normalizing slashes.
fn join_paths(prefix: &str, path: &str) -> String {
    let a = prefix.trim_matches('/');
    let b = path.trim_matches('/');
    match (a.is_empty(), b.is_empty()) {
        (true, true) => String::new(),
        (true, false) => b.to_string(),
        (false, true) => a.to_string(),
        (false, false) => format!("{a}/{b}"),
    }
}

/// Substitute a path pattern's `:param` segments from `params`, appending any
/// unused params as a deterministic query string (go_router parity).
fn substitute_pattern(pattern: &str, params: &RouteParams) -> String {
    let mut used: HashSet<&str> = HashSet::new();
    let mut path = String::new();

    for seg in pattern.split('/').filter(|s| !s.is_empty()) {
        path.push('/');
        if let Some(name) = seg.strip_prefix(':') {
            match params.get(name) {
                Some(value) => {
                    path.push_str(&encode_segment(value));
                    used.insert(name);
                }
                // A missing param leaves the literal `:name` — a wiring bug the
                // caller sees in the resulting (unmatchable) path rather than a
                // panic.
                None => path.push_str(seg),
            }
        } else {
            path.push_str(seg);
        }
    }
    if path.is_empty() {
        path.push('/');
    }

    let extras: Vec<String> = params
        .iter()
        .filter(|(k, _)| !used.contains(k.as_str()))
        .map(|(k, v)| format!("{}={}", encode_segment(k), encode_segment(v)))
        .collect();
    if extras.is_empty() {
        path
    } else {
        format!("{}?{}", path, extras.join("&"))
    }
}

/// The synthetic location an unknown named route reports to the error page.
fn named_error_location(name: &str) -> Location {
    Location::parse(&format!("/{name}"))
}

/// The default error page: a simple themed text page naming the missing location.
fn default_error_page<State: 'static>(location: &Location) -> AnyView<State> {
    any(crate::text(format!("Page not found: {}", location.path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RecordingScene;
    use frust_core::{
        BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, FrameTime, InputEvent,
        LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot,
        View, Widget,
    };
    use kurbo::{Point, Size};
    use std::cell::Cell;
    use std::rc::Rc;

    use super::super::navigator::{NavigatorView, navigator};

    fn params(pairs: &[(&str, &str)]) -> RouteParams {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // --- A sized leaf so paint-culling / stack tests can tell pages apart. ---

    struct SizedLeaf {
        size: Size,
    }
    struct SizedLeafWidget {
        size: Size,
    }
    impl<S: 'static> View<S> for SizedLeaf {
        type Element = SizedLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SizedLeafWidget {
            SizedLeafWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SizedLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for SizedLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
        }
    }
    fn sized<S: 'static>(w: f64, h: f64) -> AnyView<S> {
        any(SizedLeaf {
            size: Size::new(w, h),
        })
    }

    // --- A leaf with retained state so go(replace) vs push(retain) is provable. ---

    struct CounterView {
        observed: Rc<Cell<u32>>,
    }
    struct CounterWidget {
        count: u32,
        observed: Rc<Cell<u32>>,
    }
    impl View<()> for CounterView {
        type Element = CounterWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterWidget {
            CounterWidget {
                count: 0,
                observed: self.observed.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut CounterWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.observed = self.observed.clone();
            ChangeFlags::NONE
        }
    }
    impl Widget for CounterWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.observed.set(self.count);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                self.count += 1;
                ctx.request_redraw();
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }
    fn counter(observed: &Rc<Cell<u32>>) -> AnyView<()> {
        any(CounterView {
            observed: observed.clone(),
        })
    }

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // ================= Matcher =================

    #[test]
    fn matches_static_route() {
        let router: Router<()> = Router::new(vec![Route::new("/settings", |_| sized(10.0, 10.0))]);
        let res = router.resolve("/settings");
        assert!(res.is_matched());
        assert_eq!(res.page_count(), 1);
    }

    #[test]
    fn extracts_param() {
        let router: Router<()> = Router::new(vec![Route::new("/users/:id", |_| sized(10.0, 10.0))]);
        let res = router.resolve("/users/42");
        assert!(res.is_matched());
        assert_eq!(res.params().get("id").map(String::as_str), Some("42"));
    }

    #[test]
    fn parses_query_into_params_via_location() {
        // Query lives on the Location; the matched resolution carries the final
        // location so a page can read it.
        let router: Router<()> = Router::new(vec![Route::new("/users/:id", |_| sized(10.0, 10.0))]);
        let Resolution::Matched { location, .. } = router.resolve("/users/42?tab=posts") else {
            panic!("expected match");
        };
        assert_eq!(location.query.get("tab").map(String::as_str), Some("posts"));
    }

    #[test]
    fn nested_route_composes_and_chains() {
        let router: Router<()> = Router::new(vec![
            Route::new("/users", |_| sized(10.0, 10.0))
                .child(Route::new(":id", |_| sized(20.0, 20.0))),
        ]);
        let res = router.resolve("/users/42");
        assert!(res.is_matched());
        // Parent + child = two pages in the chain.
        assert_eq!(res.page_count(), 2);
        assert_eq!(res.params().get("id").map(String::as_str), Some("42"));
    }

    #[test]
    fn trailing_slash_still_matches() {
        let router: Router<()> = Router::new(vec![Route::new("/users/:id", |_| sized(10.0, 10.0))]);
        assert!(router.resolve("/users/42/").is_matched());
    }

    #[test]
    fn no_match_is_error() {
        let router: Router<()> = Router::new(vec![Route::new("/home", |_| sized(10.0, 10.0))]);
        let res = router.resolve("/nope");
        assert!(!res.is_matched());
        assert!(matches!(res, Resolution::Error { .. }));
    }

    // ================= Redirects =================

    #[test]
    fn per_route_redirect_follows_chain() {
        let router: Router<()> = Router::new(vec![
            Route::new("/old", |_| sized(10.0, 10.0)).redirect(|_| Some("/new".to_string())),
            Route::new("/new", |_| sized(20.0, 20.0)),
        ]);
        let Resolution::Matched { location, .. } = router.resolve("/old") else {
            panic!("expected match after redirect");
        };
        assert_eq!(location.path, "/new");
    }

    #[test]
    fn top_level_redirect_applies() {
        let router: Router<()> = Router::new(vec![
            Route::new("/login", |_| sized(10.0, 10.0)),
            Route::new("/home", |_| sized(20.0, 20.0)),
        ])
        .redirect(|loc| (loc.path == "/home").then(|| "/login".to_string()));
        let Resolution::Matched { location, .. } = router.resolve("/home") else {
            panic!("expected redirect to /login");
        };
        assert_eq!(location.path, "/login");
    }

    #[test]
    fn redirect_loop_falls_back_to_error_at_limit() {
        // Two routes redirecting to each other: the loop guard must give up at the
        // limit (5) and error rather than spin forever.
        let router: Router<()> = Router::new(vec![
            Route::new("/a", |_| sized(10.0, 10.0)).redirect(|_| Some("/b".to_string())),
            Route::new("/b", |_| sized(20.0, 20.0)).redirect(|_| Some("/a".to_string())),
        ]);
        let res = router.resolve("/a");
        assert!(
            matches!(res, Resolution::Error { .. }),
            "loop must error out"
        );
    }

    #[test]
    fn redirect_to_same_location_is_not_a_loop() {
        // A redirect returning the current location makes no progress and must be
        // ignored, not counted as a loop.
        let router: Router<()> = Router::new(vec![
            Route::new("/x", |_| sized(10.0, 10.0)).redirect(|_| Some("/x".to_string())),
        ]);
        assert!(router.resolve("/x").is_matched());
    }

    // ================= Named navigation =================

    #[test]
    fn named_resolves_params_into_path() {
        let router: Router<()> = Router::new(vec![
            Route::new("/users/:id", |_| sized(10.0, 10.0)).name("user"),
        ]);
        assert_eq!(
            router.path_for_name("user", &params(&[("id", "42")])),
            Some("/users/42".to_string())
        );
    }

    #[test]
    fn named_composes_nested_path() {
        let router: Router<()> = Router::new(vec![
            Route::new("/users", |_| sized(10.0, 10.0))
                .child(Route::new(":id", |_| sized(20.0, 20.0)).name("user")),
        ]);
        assert_eq!(
            router.path_for_name("user", &params(&[("id", "7")])),
            Some("/users/7".to_string())
        );
    }

    #[test]
    fn named_extra_params_become_query() {
        let router: Router<()> = Router::new(vec![
            Route::new("/users/:id", |_| sized(10.0, 10.0)).name("user"),
        ]);
        assert_eq!(
            router.path_for_name("user", &params(&[("id", "42"), ("tab", "posts")])),
            Some("/users/42?tab=posts".to_string())
        );
    }

    #[test]
    fn unknown_name_is_none() {
        let router: Router<()> = Router::new(vec![Route::new("/home", |_| sized(10.0, 10.0))]);
        assert!(
            router
                .path_for_name("missing", &RouteParams::new())
                .is_none()
        );
    }

    // ================= Go vs push, real navigator =================

    fn drive_paint(root: &mut RenderRoot<(), NavigatorView<()>>) -> Vec<(Point, Size)> {
        root.layout(Size::new(100.0, 100.0));
        let mut scene = RecordingScene::default();
        root.paint(&mut scene, FrameTime::ZERO);
        scene.rects
    }

    #[test]
    fn go_replaces_top_dropping_state() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed = Rc::new(Cell::new(0u32));
        let router = {
            let obs = observed.clone();
            Router::with_controller(
                &controller,
                vec![Route::new("/home", move |_| counter(&obs))],
            )
        };

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            let obs = observed.clone();
            move |_: &mut ()| {
                let obs = obs.clone();
                navigator(&ctrl, move || counter(&obs))
            }
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        // Increment the home counter.
        root.event(&mut state, &down(5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1);

        // go(/home) replaces the top with a fresh page: state dropped → counter 0.
        router.go("/home");
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 0, "go dropped the previous page's state");
    }

    #[test]
    fn push_stacks_and_retains_below() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(
            &controller,
            vec![
                Route::new("/home", |_| sized(10.0, 10.0)),
                Route::new("/detail", |_| sized(20.0, 20.0)),
            ],
        );

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        // Only the home page paints.
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(10.0, 10.0))]
        );

        // push(/detail): detail (20x20, opaque) covers home — only detail paints.
        router.push("/detail");
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(20.0, 20.0))]
        );

        // pop: home is revealed again (its pod was retained beneath detail).
        router.pop();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(10.0, 10.0))]
        );
    }

    #[test]
    fn go_to_unmatched_shows_error_page() {
        // A go() to an unmatched location drives the controller with the error
        // page rather than silently doing nothing.
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(
            &controller,
            vec![Route::new("/home", |_| sized(10.0, 10.0))],
        )
        .error_builder(|_| sized(99.0, 99.0));

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);

        router.go("/does-not-exist");
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            drive_paint(&mut root),
            vec![(Point::ZERO, Size::new(99.0, 99.0))],
            "the error page replaced the top"
        );
    }
}
