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
//! leaf (the page below is retained); [`replace`](Router::replace) swaps the top
//! for the matched leaf; [`pop`](Router::pop) pops one page.
//! [`go_named`](Router::go_named)/[`push_named`](Router::push_named) resolve a
//! named route's params into a path first. Every one drives the owned
//! [`NavigatorController`] — no reactive types here; the signal glue that feeds
//! [`handle_location`](Router::handle_location) deep links lives in the facade,
//! keeping `frust-widgets` reactive-free.
//!
//! The router is plain data + logic an app keeps in its `Component::State`
//! alongside the controller — no global registry.
//!
//! # Reaching the router from a screen
//!
//! Neither a `Router` (`Rc` page builders) nor a [`NavigatorController`]
//! (`Rc<RefCell<…>>`) can ride `provide_context`, which needs `Send + Sync`. The
//! seam that can is [`RouteNavigator`] — a plain `Send + Sync` queue of
//! [`NavRequest`] data any screen (or background task) appends to, which
//! [`pump`](Router::pump) drains and applies here on the UI thread. Get one from
//! [`route_navigator`](Router::route_navigator); the facade's
//! `RouterDeepLinks::track` pumps it every rebuild, so an app that already calls
//! `track()` gets path navigation with no extra wiring. See the
//! [`route`](super::route) module for the full rationale.
//!
//! # Params handed to a page builder
//!
//! A page's [`RouteBuilder`] receives the location's **query merged under the
//! path captures** — `/terminal?session=abc` reaches its builder with
//! `session=abc`, and a `:id` capture beats a `?id=` of the same name. This is
//! also what makes named navigation round-trip: [`path_for_name`](Router::path_for_name)
//! emits params it could not substitute into a segment as query parameters, and
//! resolution now reads them back.
//!
//! # Shell routes (nested navigators)
//!
//! [`shell_route`] binds a route's **children** to a second
//! [`NavigatorController`] the app owns: the shell route's own page is built on
//! the enclosing controller and *retained* while its children resolve onto the
//! inner one (go_router's `ShellRoute`). A resolved chain that crosses a shell
//! boundary is therefore **split** — the segment up to and including the shell
//! page applies to the enclosing controller, the segment below it to the
//! shell's inner controller — instead of flattening onto the single controller
//! the router was built with. A chain that crosses no shell still flattens
//! exactly as it always did.
//!
//! The **keep rule** is the point: when the shell page is already on the
//! enclosing stack, a navigation *within* its subtree issues **zero** ops
//! there. Replacing that page would drop the retained inner navigator and
//! every page in it, which is the whole thing a shell exists to prevent.
//! "Already placed?" is answered by the published route stack
//! ([`NavigatorController::route_stack`]) — fact, not the op queue's intent —
//! plus this router's own record of a placement it queued in this same frame
//! (see [`shell_route`] for the full table and the staleness note).
//!
//! # Deferred (documented seams)
//!
//! * **Full arbitrary-depth stack reset for `go`.** The [`NavigatorController`]
//!   exposes `push`/`pop`/`replace` only (no "pop to root"/"clear"). `go` therefore
//!   *replaces the current top* and pushes the chain's remaining pages, dropping
//!   the replaced page's state — correct for the common flat case and for the
//!   depth-1 root case. Resetting a deeper stack to a shorter chain (dropping
//!   pages *below* the top) needs a controller reset op; recorded for a later
//!   navigator revision rather than reaching across into `navigator.rs`. A shell
//!   chain does not widen this: each segment is placed with the same
//!   replace-top-and-push-the-rest rule, one controller at a time.
//! * **Shell-aware [`pop`](Router::pop).** `Router::pop` pops the router's *own*
//!   controller, shell or no shell. Popping "one page, wherever the user
//!   actually is" is back arbitration's job, not the router's — the facade's
//!   back handler routes a press to the innermost navigator that claims
//!   interest, and an inner navigator at depth 1 claims none, so the press
//!   falls through and pops the shell page itself. See [`shell_route`]'s
//!   *Back* section.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use frust_core::{AnyView, View, any};

use super::navigator::{NavigatorController, NavigatorId, PushOptions, ReplaceOptions};
use super::path::{Location, PathPattern, RouteParams, encode_segment};
use super::route::{NavRequest, RouteNavigator};

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
    /// Set only by [`shell_route`]: the controller this route's **children**
    /// resolve onto. `None` (every route built with [`Route::new`]) means the
    /// children resolve onto the same controller this route's own page does,
    /// i.e. the chain flattens.
    shell: Option<NavigatorController<State>>,
}

impl<State: 'static> Route<State> {
    /// A route matching `path` (a pattern like `/users/:id`, or a relative child
    /// path like `:id`), rendered by `builder`. `builder` may return any
    /// [`View`]; it is erased here.
    ///
    /// ```
    /// use frust_widgets::{Route, text};
    /// let route: Route<()> = Route::new("/users/:id", |params| {
    ///     text(format!("user {}", params.get("id").map_or("?", String::as_str)))
    /// });
    /// # let _ = route;
    /// ```
    pub fn new<V: View<State>>(
        path: impl Into<String>,
        builder: impl Fn(&RouteParams) -> V + 'static,
    ) -> Self {
        Route {
            path: path.into(),
            name: None,
            builder: Rc::new(move |params: &RouteParams| any(builder(params))),
            redirect: None,
            children: Vec::new(),
            shell: None,
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

/// A **shell route**: a pathless route whose `children` resolve onto `inner`, a
/// second [`NavigatorController`] the app owns, while this route's own page
/// stays retained on the enclosing stack (go_router's `ShellRoute`).
///
/// `builder` builds the shell page — chrome plus the inner navigator, e.g.
/// `scaffold(navigator(&inner, || …)).app_bar(…)` — and **must** mount a
/// [`navigator`](super::navigator::navigator) driven by the *same* `inner`
/// clone; that navigator is where the children land.
///
/// ```ignore
/// // The app owns `inner` in its `Component::State`, exactly like the outer one.
/// Router::with_controller(&outer, vec![
///     Route::new("/", connect_page),               // declared BEFORE the shell
///     shell_route(&inner, shell_page, vec![
///         Route::new("/sessions", sessions_page),
///         Route::new("/terminal", terminal_page),
///     ]),
/// ])
/// ```
///
/// # Pathless, and why order matters
///
/// A shell route consumes no path segments (like go_router's `ShellRoute`,
/// which has no `path` at all), so its children keep flat public paths. A
/// zero-consuming route also matches the *empty* segment list, so a root route
/// (`/`) must be declared **before** the shell — [`Router::resolve`] takes the
/// first match. To scope a shell under a prefix, nest it: a
/// `Route::new("/dash").child(shell_route(…))` puts `/dash`'s own page on the
/// enclosing stack ahead of the shell page.
///
/// # How a chain that crosses this route is applied
///
/// The resolved chain splits at the boundary. With the shell page already on
/// the enclosing stack — the **keep rule** — the enclosing controller gets
/// **zero ops**, because replacing that page would drop the retained inner
/// navigator and every page in it:
///
/// | Verb | Enclosing controller | Inner controller |
/// |---|---|---|
/// | [`go`](Router::go) | shell already placed → **keep** (zero ops); else replace the top with the shell page and push the rest | replace the top with the leaf |
/// | [`push`](Router::push) | shell already placed → **keep**; else *push* the shell page (the page below — a `/` connect screen, say — is retained) | push the leaf |
/// | [`replace`](Router::replace) | as `go`'s rule | replace the top with the leaf |
/// | [`pop`](Router::pop) | pops the router's own controller — see the [module docs](self)' deferred note | — |
///
/// One structural op per controller per navigation, so nothing here rests on
/// stacking two transitions in one `apply_ops` pass.
///
/// **A placement resets the verb below it.** When the shell page has to be
/// placed, the inner navigator is brand new — its stack is just the root page
/// its `navigator(…)` call names — so there is no in-shell history to stack
/// onto and every segment below the placement applies as a `go` (replace)
/// whatever the caller asked for. That is what makes `push("/sessions")` from a
/// bar-less `/` land as `[/, shell]` outside and `[sessions]` (depth 1) inside,
/// so a back press at `/sessions` leaves the shell instead of popping to a
/// placeholder root.
///
/// # Matched state reaching the shell page
///
/// `builder` receives the resolved chain's merged [`RouteParams`] like any
/// other route — but only as of the navigation that *placed* it. A navigation
/// within the shell issues no op on the enclosing controller by design, so the
/// shell page is neither rebuilt from a new builder nor restamped: its
/// published route entry keeps naming the location that placed it. Live
/// in-shell state is read from the **inner** navigator instead — its own
/// [`route_stack`](NavigatorController::route_stack) (or the facade's
/// `RouteObserver` over it), which is authoritative at every publish. Chrome
/// inside the shell page (a title bar) reads that, never the enclosing stack.
///
/// # Deep links landing mid-shell
///
/// A `go` that runs before the shell page exists — a cold-start deep link
/// resolved from `Component::init` — queues the inner segment's ops on `inner`
/// while it has no widget at all. They are drained by the inner navigator's
/// *first* `build` (the same pre-first-frame drain a top-level controller
/// gets), so the shell's first painted frame is already the linked child: no
/// flash, no second navigation. If a shell page defers mounting its navigator
/// (rendering a loading state first), the ops simply wait on the queue and land
/// on whichever build mounts it.
///
/// # Back
///
/// Nothing here wires back. The shell page's builder mounts the inner
/// navigator, so the inner navigator wires *after* the enclosing one and the
/// facade's innermost-first arbitration reaches it first: back pops the inner
/// stack while it is poppable, and an inner navigator at depth 1 claims no
/// interest, so the press falls through and pops the shell page itself.
pub fn shell_route<State: 'static, V: View<State>>(
    inner: &NavigatorController<State>,
    builder: impl Fn(&RouteParams) -> V + 'static,
    children: Vec<Route<State>>,
) -> Route<State> {
    Route {
        path: String::new(),
        name: None,
        builder: Rc::new(move |params: &RouteParams| any(builder(params))),
        redirect: None,
        children,
        shell: Some(inner.clone()),
    }
}

/// One resolved page in a [`Resolution::Matched`] chain: the route's builder,
/// the params handed to it (query merged under the path captures — see the
/// [module docs](self)), and the resolved location they came from.
/// [`build`](ResolvedPage::build) produces the page view; the router wraps it in
/// a `Fn() -> AnyView` for the controller.
pub struct ResolvedPage<State: 'static> {
    builder: RouteBuilder<State>,
    params: RouteParams,
    location: Location,
    /// The [`shell_route`] binding of the route this page came from: the
    /// controller every page *below* it in the chain applies to. `None` for
    /// every ordinary route, which is what keeps a non-shell chain flattening
    /// onto one controller.
    shell: Option<NavigatorController<State>>,
}

impl<State: 'static> ResolvedPage<State> {
    /// Build the page view against its params.
    pub fn build(&self) -> AnyView<State> {
        (self.builder)(&self.params)
    }

    /// The params this page's builder receives.
    pub fn params(&self) -> &RouteParams {
        &self.params
    }

    /// The final (post-redirect) location this page resolved from — the raw
    /// [`Location`] behind [`params`](Self::params), for a caller that needs the
    /// path/query split rather than the merged map.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// Turn this page into the `Fn() -> AnyView` closure a
    /// [`NavigatorController`] op takes (re-run each rebuild).
    fn into_page_builder(self) -> impl Fn() -> AnyView<State> + 'static {
        let ResolvedPage {
            builder, params, ..
        } = self;
        move || (builder)(&params)
    }
}

/// The outcome of [`Router::resolve`]: either a matched chain of pages (with the
/// final resolved location and merged params) or an error fallback.
pub enum Resolution<State: 'static> {
    /// The location matched a route chain. `pages` is the chain root→leaf; for a
    /// flat route it holds a single page. `params` is the merged param map (the
    /// location's query merged **under** the path captures — a capture wins a
    /// name collision), `location` the final (post-redirect) location.
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

    /// The merged params (query under path captures), or an empty map for an
    /// error resolution.
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

/// Which navigation verb a resolved chain is being applied under — the row
/// selector in [`shell_route`]'s application table, and what lets `go`/`push`/
/// `replace` share one chain-splitting path instead of three copies of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    Go,
    Push,
    Replace,
}

/// One piece of a chain split at its [`shell_route`] boundaries: the pages that
/// apply to `controller`. Segment 0's controller is the router's own; each
/// later segment's is the inner controller of the shell that opened it.
struct Segment<State: 'static> {
    controller: NavigatorController<State>,
    pages: Vec<ResolvedPage<State>>,
}

/// A declarative router over a [`navigator`](super::navigator). See the [module
/// docs](self).
pub struct Router<State: 'static> {
    routes: Vec<Route<State>>,
    controller: NavigatorController<State>,
    redirect: Option<Redirect>,
    error_builder: ErrorBuilder<State>,
    redirect_limit: usize,
    /// The `Send + Sync` request queue screens navigate through (see the
    /// [module docs](self)' "Reaching the router from a screen"). Owned here,
    /// handed out by clone; drained by [`pump`](Self::pump).
    route_nav: RouteNavigator,
    /// Shell pages this router has **queued** but whose target controller has
    /// not published them yet, keyed by the shell's inner-controller id and
    /// valued by that target's [`route_generation`](NavigatorController::route_generation)
    /// at queue time.
    ///
    /// The keep rule reads committed state (the published route stack), which
    /// is the right authority — but it leaves one gap the router alone can
    /// close: two shell-crossing navigations applied in the *same* frame, before
    /// any rebuild, would each see a stack without the shell page and each queue
    /// one, leaving two shell pages driving a single inner controller. An entry
    /// here is honoured only while the recorded generation still matches, so it
    /// self-invalidates on the very publish that makes the stack authoritative —
    /// it is a note about this router's own queued intent, never a second copy
    /// of the stack.
    pending_shells: RefCell<HashMap<NavigatorId, u64>>,
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
            route_nav: RouteNavigator::new(),
            pending_shells: RefCell::new(HashMap::new()),
        }
    }

    /// Set a top-level redirect, consulted before matching on every resolution.
    pub fn redirect(mut self, redirect: impl Fn(&Location) -> Option<String> + 'static) -> Self {
        self.redirect = Some(Rc::new(redirect));
        self
    }

    /// Replace the error-page builder (default: a simple themed "not found" page).
    /// `error_builder` may return any [`View`]; it is erased here.
    pub fn error_builder<V: View<State>>(
        mut self,
        error_builder: impl Fn(&Location) -> V + 'static,
    ) -> Self {
        self.error_builder = Rc::new(move |location: &Location| any(error_builder(location)));
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

            // 4. Matched, no redirect: build the page chain. A page builder
            //    sees the location's query merged UNDER the path captures — a
            //    `:id` capture beats a `?id=` of the same name — so
            //    `/terminal?session=abc` can read its own parameter and a
            //    named-route path built with extra params round-trips back
            //    through `resolve` (see the module docs).
            let mut merged = loc.query.clone();
            merged.extend(params);
            let pages = chain
                .iter()
                .map(|route| ResolvedPage {
                    builder: route.builder.clone(),
                    params: merged.clone(),
                    location: loc.clone(),
                    shell: route.shell.clone(),
                })
                .collect();
            return Resolution::Matched {
                location: loc,
                params: merged,
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

    /// The [`RouteNavigator`] screens navigate through: a `Send + Sync` handle
    /// safe to hold under `provide_context`, in a callback, or on a background
    /// thread. Requests queued on it are applied by this router's next
    /// [`pump`](Self::pump) — see the [module docs](self)' "Reaching the router
    /// from a screen".
    pub fn route_navigator(&self) -> RouteNavigator {
        self.route_nav.clone()
    }

    /// Drain the [`route_navigator`](Self::route_navigator)'s queue and apply
    /// each request, in order. Idempotent when the queue is empty, so it is meant
    /// to be called unconditionally once per rebuild (the facade's
    /// `RouterDeepLinks::track` does exactly that). Must run on the UI thread —
    /// it drives the `Rc`-backed [`NavigatorController`]; queuing is what is
    /// thread-free.
    pub fn pump(&self) {
        for request in self.route_nav.drain() {
            match request {
                NavRequest::Go(location) => self.go(&location),
                NavRequest::Push(location) => self.push(&location),
                NavRequest::Replace(location) => self.replace(&location),
                NavRequest::Pop => self.pop(),
                NavRequest::GoNamed { name, params } => self.go_named(&name, &params),
                NavRequest::PushNamed { name, params } => self.push_named(&name, &params),
            }
        }
    }

    /// Reset the stack to the matched chain (replace semantics — the current top's
    /// state is dropped). See the [module docs](self)'s deferred note on
    /// arbitrary-depth reset.
    ///
    /// Each resolved page is stamped with its own [`ResolvedPage::location`]
    /// (`R-B1`) — [`NavigatorController::route_stack`] tracks the
    /// resulting page-to-route mapping, so no consumer needs to re-derive it.
    ///
    /// A chain crossing a [`shell_route`] boundary is split across the two
    /// controllers instead, under the keep rule — see [`shell_route`]'s table.
    pub fn go(&self, location: &str) {
        self.apply(self.resolved(location), Verb::Go);
    }

    /// Push the matched leaf page onto the stack (the page below is retained).
    ///
    /// Stamps the leaf's [`ResolvedPage::location`] — see [`go`](Self::go)'s doc.
    /// Across a [`shell_route`] boundary the leaf lands on the shell's inner
    /// controller — see [`shell_route`]'s table.
    pub fn push(&self, location: &str) {
        self.apply(self.resolved(location), Verb::Push);
    }

    /// Replace the top page with the matched leaf (the top's state is dropped;
    /// the stack depth is unchanged). [`go`](Self::go)'s single-page case, minus
    /// the chain push — the op a [`NavRequest::Replace`] applies.
    ///
    /// Stamps the leaf's [`ResolvedPage::location`] — see [`go`](Self::go)'s doc.
    /// Across a [`shell_route`] boundary the replaced top is the shell's inner
    /// one — see [`shell_route`]'s table.
    pub fn replace(&self, location: &str) {
        self.apply(self.resolved(location), Verb::Replace);
    }

    /// Pop the top page of the router's **own** controller (a no-op on the root
    /// page — see [`NavigatorController::pop`]).
    ///
    /// Deliberately not shell-aware: popping "one page, wherever the user
    /// actually is" is back arbitration's job (the facade's back handler routes
    /// a press innermost-first), not the router's. See the [module docs](self)'
    /// deferred note and [`shell_route`]'s *Back* section.
    pub fn pop(&self) {
        self.controller.pop();
    }

    // --- Chain application (flat and shell-split) ---

    /// Apply a [`Resolution`] under `verb`. A chain that crosses no shell
    /// boundary is one segment and takes [`apply_flat`](Self::apply_flat) — the
    /// flatten-onto-one-controller behaviour, unchanged. A chain that *does*
    /// cross one takes [`apply_shell_chain`](Self::apply_shell_chain).
    fn apply(&self, resolution: Resolution<State>, verb: Verb) {
        match resolution {
            Resolution::Matched { pages, .. } => {
                let mut segments = self.split_chain(pages);
                if segments.len() == 1 {
                    let segment = segments.pop().expect("a chain has at least one segment");
                    Self::apply_flat(&segment.controller, segment.pages, verb);
                } else {
                    self.apply_shell_chain(segments, verb);
                }
            }
            Resolution::Error { location } => match verb {
                Verb::Push => self.controller.push(self.error_page_builder(location)),
                Verb::Go | Verb::Replace => {
                    self.controller.replace(self.error_page_builder(location))
                }
            },
        }
    }

    /// Cut the resolved chain after every page whose route carries a
    /// [`shell_route`] binding: segment 0 applies to this router's own
    /// controller, segment 1 to the first shell's inner controller, and so on.
    /// A chain crossing no shell yields exactly one segment.
    ///
    /// A shell route matched as the chain *leaf* (its own path, with no child
    /// consuming the rest) opens a trailing segment with no pages — kept, so
    /// the shell page is still placed while its inner navigator is left showing
    /// whatever it already had.
    fn split_chain(&self, pages: Vec<ResolvedPage<State>>) -> Vec<Segment<State>> {
        let mut segments = vec![Segment {
            controller: self.controller.clone(),
            pages: Vec::new(),
        }];
        for page in pages {
            let inner = page.shell.clone();
            segments
                .last_mut()
                .expect("segments is seeded with the outer segment")
                .pages
                .push(page);
            if let Some(inner) = inner {
                segments.push(Segment {
                    controller: inner,
                    pages: Vec::new(),
                });
            }
        }
        segments
    }

    /// Apply a chain that crosses at least one shell boundary: each segment to
    /// its own controller, under the keep rule (see [`shell_route`]'s table).
    fn apply_shell_chain(&self, segments: Vec<Segment<State>>, verb: Verb) {
        // The inner controller each boundary opens, indexed by boundary — the
        // identity the keep rule compares an already-placed shell page against.
        let shells: Vec<NavigatorId> = segments
            .iter()
            .skip(1)
            .map(|segment| segment.controller.id())
            .collect();
        let last = segments.len() - 1;
        // While every enclosing shell page was kept, the caller's verb applies
        // as written. The first placement flips this: everything below a
        // freshly placed shell page is a new subtree with no history to stack
        // onto, so it resets (see `shell_route`'s "a placement resets the verb
        // below it").
        let mut enclosing_kept = true;

        for (index, segment) in segments.into_iter().enumerate() {
            let effective = if enclosing_kept { verb } else { Verb::Go };
            if index == last {
                Self::apply_flat(&segment.controller, segment.pages, effective);
                continue;
            }

            let shell = shells[index];
            if enclosing_kept && self.shell_is_placed(&segment.controller, index, shell) {
                continue;
            }
            enclosing_kept = false;
            self.pending_shells
                .borrow_mut()
                .insert(shell, segment.controller.route_generation());
            Self::place_segment(&segment.controller, segment.pages, effective);
        }
    }

    /// The keep rule's question: is the page of the shell that opens boundary
    /// `boundary` already on `target`'s stack?
    ///
    /// Answered against the **published** route stack — the committed fact, per
    /// `route_state`'s intent-vs-fact split — by re-matching each entry's
    /// location through the route table and asking whether that chain crosses
    /// the same shell at the same boundary. Re-matching (rather than comparing
    /// locations) is what keeps a pathless shell honest: the shell page is
    /// stamped with the location that placed it, and every one of its children
    /// resolves through the same shell.
    ///
    /// "On the stack", not "on top": a page the app pushed *over* the shell
    /// leaves the shell page retained beneath it, and placing a second one
    /// there would put two live navigators on one controller. The in-shell
    /// navigation lands under the covering page instead, and is what the user
    /// sees when they pop back to it.
    ///
    /// Matching is redirect-free ([`match_chain`](Self::match_chain), not
    /// [`resolve`](Self::resolve)): this identifies the route that *produced* an
    /// existing page, which a redirect installed later must not reinterpret.
    fn shell_is_placed(
        &self,
        target: &NavigatorController<State>,
        boundary: usize,
        shell: NavigatorId,
    ) -> bool {
        let stack = target.route_stack();
        let committed = stack
            .entries()
            .iter()
            .flatten()
            .any(|location| self.shell_at(location, boundary) == Some(shell));
        if committed {
            return true;
        }
        // Queued but not yet published — this router's own placement from
        // earlier in the same frame (see `pending_shells`).
        self.pending_shells.borrow().get(&shell) == Some(&stack.generation())
    }

    /// The inner-controller id of the `boundary`-th shell the chain matching
    /// `location` crosses, or `None` if it crosses fewer.
    fn shell_at(&self, location: &Location, boundary: usize) -> Option<NavigatorId> {
        let (chain, _) = self.match_chain(location)?;
        chain
            .iter()
            .filter_map(|route| route.shell.as_ref())
            .nth(boundary)
            .map(|controller| controller.id())
    }

    /// Apply one segment's pages to `controller` with the flat, single-controller
    /// semantics `go`/`push`/`replace` have always had — the behaviour a chain
    /// crossing no shell keeps byte-for-byte.
    fn apply_flat(
        controller: &NavigatorController<State>,
        pages: Vec<ResolvedPage<State>>,
        verb: Verb,
    ) {
        match verb {
            Verb::Go => {
                let mut pages = pages.into_iter();
                if let Some(first) = pages.next() {
                    let route = first.location().clone();
                    controller.replace_with_options(
                        first.into_page_builder(),
                        ReplaceOptions::opaque().route(route),
                    );
                }
                for page in pages {
                    let route = page.location().clone();
                    controller.push_with_options(
                        page.into_page_builder(),
                        PushOptions::opaque().route(route),
                    );
                }
            }
            Verb::Push => {
                if let Some(leaf) = pages.into_iter().next_back() {
                    let route = leaf.location().clone();
                    controller.push_with_options(
                        leaf.into_page_builder(),
                        PushOptions::opaque().route(route),
                    );
                }
            }
            Verb::Replace => {
                if let Some(leaf) = pages.into_iter().next_back() {
                    let route = leaf.location().clone();
                    controller.replace_with_options(
                        leaf.into_page_builder(),
                        ReplaceOptions::opaque().route(route),
                    );
                }
            }
        }
    }

    /// Place a shell-carrying segment (its pages end with the shell page) on
    /// `controller`. A `push` stacks the whole segment so the page it came from
    /// is retained beneath the shell; a `go`/`replace` applies the flat `go`
    /// rule (replace the top, push the rest).
    fn place_segment(
        controller: &NavigatorController<State>,
        pages: Vec<ResolvedPage<State>>,
        verb: Verb,
    ) {
        match verb {
            Verb::Push => {
                for page in pages {
                    let route = page.location().clone();
                    controller.push_with_options(
                        page.into_page_builder(),
                        PushOptions::opaque().route(route),
                    );
                }
            }
            Verb::Go | Verb::Replace => Self::apply_flat(controller, pages, Verb::Go),
        }
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
            None => {
                let location = named_error_location(name);
                self.route_nav.set_location(location.clone());
                self.controller.replace(self.error_page_builder(location));
            }
        }
    }

    /// Resolve `name` + `params` into a path, then [`push`](Self::push) it. An
    /// unknown name routes to the error page.
    pub fn push_named(&self, name: &str, params: &RouteParams) {
        match self.path_for_name(name, params) {
            Some(path) => self.push(&path),
            None => {
                let location = named_error_location(name);
                self.route_nav.set_location(location.clone());
                self.controller.push(self.error_page_builder(location));
            }
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

    /// [`resolve`](Self::resolve) plus the one side effect the navigating
    /// methods share: publishing the resolved (post-redirect) location on the
    /// [`RouteNavigator`], so `location()` reports where the router actually
    /// went — including the location an unmatched link errored on.
    fn resolved(&self, raw: &str) -> Resolution<State> {
        let resolution = self.resolve(raw);
        let location = match &resolution {
            Resolution::Matched { location, .. } | Resolution::Error { location } => location,
        };
        self.route_nav.set_location(location.clone());
        resolution
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
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::navigator::{NavigatorView, navigator};
    use super::super::route_state::NavChange;

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
    fn sized<S: 'static>(w: f64, h: f64) -> impl View<S> {
        SizedLeaf {
            size: Size::new(w, h),
        }
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
    // erasure: keep borrow escapes the fn (callers pass it through a 'static route closure)
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

    /// Run a page builder and hand back the params it actually received — the
    /// only thing that proves the query merge reaches a page (rather than just
    /// the `Resolution`'s own map).
    fn builder_params(resolution: &Resolution<()>) -> RouteParams {
        let Resolution::Matched { pages, .. } = resolution else {
            panic!("expected match");
        };
        let leaf = pages.last().expect("a matched chain has a leaf");
        let _view = leaf.build();
        leaf.params().clone()
    }

    #[test]
    fn query_reaches_the_page_builder() {
        // R47: `/terminal?session=<id>` must be able to read its own parameter.
        let seen: Rc<RefCell<RouteParams>> = Rc::new(RefCell::new(RouteParams::new()));
        let recorder = seen.clone();
        let router: Router<()> = Router::new(vec![Route::new("/terminal", move |p| {
            *recorder.borrow_mut() = p.clone();
            sized(10.0, 10.0)
        })]);

        let resolution = router.resolve("/terminal?session=abc");
        assert_eq!(
            builder_params(&resolution)
                .get("session")
                .map(String::as_str),
            Some("abc")
        );
        assert_eq!(
            seen.borrow().get("session").map(String::as_str),
            Some("abc"),
            "the builder itself receives the query parameter"
        );
    }

    #[test]
    fn path_capture_beats_a_colliding_query_key() {
        // The merge order is query UNDER captures: `/users/42?id=99` is 42.
        let router: Router<()> = Router::new(vec![Route::new("/users/:id", |_| sized(10.0, 10.0))]);
        let resolution = router.resolve("/users/42?id=99");
        assert_eq!(
            builder_params(&resolution).get("id").map(String::as_str),
            Some("42")
        );
        assert_eq!(
            resolution.params().get("id").map(String::as_str),
            Some("42")
        );
    }

    #[test]
    fn named_extra_params_round_trip_through_resolve() {
        // `path_for_name` emits unconsumed params as query; before R47 resolve
        // dropped them again, so `push_named(name, {id, tab})` lost `tab`.
        let router: Router<()> = Router::new(vec![
            Route::new("/users/:id", |_| sized(10.0, 10.0)).name("user"),
        ]);
        let path = router
            .path_for_name("user", &params(&[("id", "42"), ("tab", "posts")]))
            .expect("named route");
        let received = builder_params(&router.resolve(&path));
        assert_eq!(received.get("id").map(String::as_str), Some("42"));
        assert_eq!(received.get("tab").map(String::as_str), Some("posts"));
    }

    #[test]
    fn resolved_page_exposes_its_location() {
        let router: Router<()> = Router::new(vec![Route::new("/users/:id", |_| sized(10.0, 10.0))]);
        let Resolution::Matched { pages, .. } = router.resolve("/users/42?tab=posts") else {
            panic!("expected match");
        };
        let leaf = pages.last().expect("leaf");
        assert_eq!(leaf.location().path, "/users/42");
        assert_eq!(
            leaf.location().query.get("tab").map(String::as_str),
            Some("posts"),
            "the raw location keeps path and query separate"
        );
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

    // ================= Route-state stamping =================

    #[test]
    fn go_push_replace_all_stamp_the_resolved_location_onto_the_route_stack() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(
            &controller,
            vec![
                Route::new("/home", |_| sized(10.0, 10.0)),
                Route::new("/detail", |_| sized(20.0, 20.0)),
                Route::new("/other", |_| sized(30.0, 30.0)),
            ],
        );

        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized(10.0, 10.0))
        };
        let mut state = ();
        // No `.root_route(...)`: the very first page is seeded by `router.go`
        // below, not the navigator's own initial builder.
        root.rebuild(&mut app, &mut state);

        router.go("/home");
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.route_stack().entries(),
            &[Some(Location::parse("/home"))],
            "Router::go stamps the resolved location via ReplaceOptions::route"
        );

        router.push("/detail");
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.route_stack().entries(),
            &[
                Some(Location::parse("/home")),
                Some(Location::parse("/detail"))
            ],
            "Router::push stamps the resolved location via PushOptions::route"
        );
        assert_eq!(controller.route_stack().change(), NavChange::Push);

        router.replace("/other");
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.route_stack().entries(),
            &[
                Some(Location::parse("/home")),
                Some(Location::parse("/other"))
            ],
            "Router::replace stamps the resolved location via ReplaceOptions::route"
        );
        assert_eq!(controller.route_stack().change(), NavChange::Replace);
    }

    // ================= RouteNavigator seam (pump) =================

    /// The build closure [`RenderRoot::rebuild`] drives, boxed so
    /// [`PumpHarness`] can store it as a field.
    type Build = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

    /// A router + navigator harness for the pump tests: the same shape the
    /// other stack tests use, bundled so a test can queue → pump → rebuild →
    /// paint in one line each.
    struct PumpHarness {
        router: Router<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: Build,
        state: (),
    }

    impl PumpHarness {
        fn new(routes: Vec<Route<()>>) -> Self {
            let controller: NavigatorController<()> = NavigatorController::new();
            // A GPU/text-free error page: the default one builds a `Text`,
            // which panics without a threaded `TextContext`.
            let router =
                Router::with_controller(&controller, routes).error_builder(|_| sized(99.0, 99.0));
            let app: Build =
                Box::new(move |_: &mut ()| navigator(&controller, || sized(10.0, 10.0)));
            let mut harness = PumpHarness {
                router,
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

        /// One frame: pump the queue (as the facade's `track()` does at the top
        /// of `build`), rebuild, paint.
        fn frame(&mut self) -> Vec<(Point, Size)> {
            self.router.pump();
            self.rebuild();
            drive_paint(&mut self.root)
        }
    }

    fn pump_routes() -> Vec<Route<()>> {
        vec![
            Route::new("/home", |_| sized(10.0, 10.0)),
            Route::new("/detail", |_| sized(20.0, 20.0)),
            Route::new("/other", |_| sized(30.0, 30.0)),
        ]
    }

    #[test]
    fn pump_applies_a_queued_request_on_the_next_frame() {
        let mut h = PumpHarness::new(pump_routes());
        let nav = h.router.route_navigator();
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(10.0, 10.0))]);

        // Recorded the way an event handler would: through the context-safe
        // handle, with no router in sight.
        nav.push("/detail");
        assert_eq!(
            h.frame(),
            vec![(Point::ZERO, Size::new(20.0, 20.0))],
            "a request queued between frames lands on the very next one"
        );

        nav.pop();
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(10.0, 10.0))]);
    }

    #[test]
    fn pump_applies_every_request_variant_in_order() {
        let mut h = PumpHarness::new(pump_routes());
        let nav = h.router.route_navigator();

        nav.push("/detail");
        nav.replace("/other");
        assert_eq!(
            h.frame(),
            vec![(Point::ZERO, Size::new(30.0, 30.0))],
            "push then replace leaves /other on top, not /detail"
        );
        // The replace swapped the top rather than stacking: one pop is back home.
        nav.pop();
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(10.0, 10.0))]);

        nav.go("/detail");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(20.0, 20.0))]);
    }

    #[test]
    fn pump_applies_named_requests_with_their_params() {
        let seen: Rc<RefCell<RouteParams>> = Rc::new(RefCell::new(RouteParams::new()));
        let recorder = seen.clone();
        let mut h = PumpHarness::new(vec![
            Route::new("/home", |_| sized(10.0, 10.0)),
            Route::new("/users/:id", move |p| {
                *recorder.borrow_mut() = p.clone();
                sized(20.0, 20.0)
            })
            .name("user"),
        ]);
        let nav = h.router.route_navigator();

        nav.push_named("user", params(&[("id", "42"), ("tab", "posts")]));
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(20.0, 20.0))]);
        // Round-trip: the extra param rode the generated query string back
        // through `resolve` into the page's own params (R47).
        assert_eq!(seen.borrow().get("id").map(String::as_str), Some("42"));
        assert_eq!(seen.borrow().get("tab").map(String::as_str), Some("posts"));
    }

    #[test]
    fn pump_is_a_no_op_when_the_queue_is_empty() {
        let mut h = PumpHarness::new(pump_routes());
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(10.0, 10.0))]);
        // Called unconditionally every rebuild — repeated pumps must not
        // re-apply the last request.
        for _ in 0..3 {
            assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(10.0, 10.0))]);
        }
    }

    #[test]
    fn a_request_from_another_thread_pumps_normally() {
        // The payoff of the `Send + Sync` handle shape: the handle crosses a
        // thread boundary, and the resolution still happens on the UI thread.
        let mut h = PumpHarness::new(pump_routes());
        let nav = h.router.route_navigator();
        let woke = Arc::new(AtomicUsize::new(0));
        let counter = woke.clone();
        nav.set_waker(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        std::thread::spawn(move || nav.push("/detail"))
            .join()
            .expect("off-thread request must not panic");

        assert_eq!(
            woke.load(Ordering::SeqCst),
            1,
            "the waker asked for a frame"
        );
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(20.0, 20.0))]);
    }

    #[test]
    fn route_navigator_publishes_the_resolved_location() {
        let mut h = PumpHarness::new(pump_routes());
        let nav = h.router.route_navigator();
        assert!(nav.location().is_none(), "nothing resolved yet");

        nav.push("/detail?tab=posts");
        let _ = h.frame();
        let loc = nav.location().expect("published after the pump");
        assert_eq!(loc.path, "/detail");
        assert_eq!(loc.query.get("tab").map(String::as_str), Some("posts"));

        // An unmatched location still reports where the router ended up.
        nav.go("/nope");
        let _ = h.frame();
        assert_eq!(nav.location().expect("published").path, "/nope");
    }

    #[test]
    fn route_navigator_clones_share_one_queue() {
        // Every `route_navigator()` call hands out the same underlying queue, so
        // a screen holding an old clone still reaches the same router.
        let mut h = PumpHarness::new(pump_routes());
        let first = h.router.route_navigator();
        let second = h.router.route_navigator();
        first.push("/detail");
        second.replace("/other");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(30.0, 30.0))]);
    }

    // ================= Shell routes =================

    /// A leaf counting how many times it was **built** (never how often it was
    /// rebuilt) — the keep rule's tripwire. A retained widget is rebuilt in
    /// place; one whose page was replaced is built again from scratch, taking
    /// the shell's inner navigator (and every page in it) with it.
    struct BuildCounter {
        builds: Rc<Cell<u32>>,
    }
    struct BuildCounterWidget;
    impl<S: 'static> View<S> for BuildCounter {
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

    /// The shell shape this whole construct exists for: a `/` root declared
    /// **before** a pathless shell (so it keeps the empty segment list) whose
    /// children are the screens that live inside the shell's chrome.
    struct ShellHarness {
        router: Router<()>,
        outer: NavigatorController<()>,
        inner: NavigatorController<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: Build,
        /// Builds of the shell page's chrome — see [`BuildCounter`].
        shell_builds: Rc<Cell<u32>>,
    }

    impl ShellHarness {
        fn new() -> Self {
            let outer: NavigatorController<()> = NavigatorController::new();
            let inner: NavigatorController<()> = NavigatorController::new();
            let shell_builds = Rc::new(Cell::new(0u32));

            let shell_page = {
                let inner = inner.clone();
                let builds = shell_builds.clone();
                move |_: &RouteParams| {
                    let inner = inner.clone();
                    let builds = builds.clone();
                    any(crate::column()
                        .child(BuildCounter { builds })
                        .child(navigator(&inner, || sized(11.0, 11.0))))
                }
            };
            let routes = vec![
                Route::new("/", |_| sized(10.0, 10.0)),
                shell_route(
                    &inner,
                    shell_page,
                    vec![
                        Route::new("/sessions", |_| sized(20.0, 20.0)),
                        Route::new("/terminal", |_| sized(30.0, 30.0)),
                    ],
                ),
            ];
            let router =
                Router::with_controller(&outer, routes).error_builder(|_| sized(99.0, 99.0));
            let app: Build = {
                let c = outer.clone();
                Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
            };
            let mut harness = ShellHarness {
                router,
                outer,
                inner,
                root: RenderRoot::new(),
                app,
                shell_builds,
            };
            harness.rebuild();
            harness
        }

        fn rebuild(&mut self) {
            self.root.rebuild(&mut self.app, &mut ());
        }

        /// The locations each controller's published stack currently holds.
        fn stacks(&self) -> (Vec<String>, Vec<String>) {
            (paths(&self.outer), paths(&self.inner))
        }
    }

    /// The published route stack of `controller` as plain paths (a route-less
    /// page reads as `-`), which is what every shell assertion below compares.
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

    #[test]
    fn a_shell_chain_splits_the_outer_page_from_the_inner_leaf() {
        let mut h = ShellHarness::new();
        h.router.go("/terminal");
        h.rebuild();

        let (outer, inner) = h.stacks();
        assert_eq!(
            outer,
            vec!["/terminal".to_string()],
            "the outer controller got the SHELL page (stamped with the location \
             that placed it), never the leaf"
        );
        assert_eq!(
            inner,
            vec!["/terminal".to_string()],
            "the leaf landed on the shell's inner controller"
        );
        assert_eq!(h.inner.depth(), 1, "the inner leaf replaced its root");
        assert!(h.inner.is_mounted(), "the shell page mounted its navigator");
        assert_eq!(h.shell_builds.get(), 1, "the shell page was built once");
    }

    #[test]
    fn a_sibling_navigation_inside_the_shell_issues_zero_outer_ops() {
        // THE keep rule: re-navigating within the shell's subtree must not
        // touch the outer stack, because replacing the shell page would drop
        // the retained inner navigator and every page in it.
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();
        let outer_generation = h.outer.route_generation();
        assert_eq!(h.shell_builds.get(), 1);

        h.router.go("/terminal");
        h.rebuild();

        assert_eq!(
            h.outer.route_generation(),
            outer_generation,
            "no outer op ran at all — an unchanged stack publishes nothing"
        );
        assert_eq!(
            h.shell_builds.get(),
            1,
            "the shell page's widget was retained, not rebuilt"
        );
        assert_eq!(
            h.stacks().1,
            vec!["/terminal".to_string()],
            "only the inner navigator moved"
        );
        assert_eq!(
            h.stacks().0,
            vec!["/sessions".to_string()],
            "the outer entry keeps naming the location that PLACED the shell — \
             live in-shell state is read from the inner navigator"
        );
    }

    #[test]
    fn a_push_inside_a_placed_shell_stacks_on_the_inner_navigator() {
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();

        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(h.outer.depth(), 1, "the outer stack never moved");
        assert_eq!(
            h.stacks().1,
            vec!["/sessions".to_string(), "/terminal".to_string()],
            "the leaf stacked inside the shell"
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn a_push_that_must_place_the_shell_retains_the_page_below_and_resets_the_inner() {
        // muxr's connect → shell shape: the page the user came from stays on
        // the outer stack (so back leaves the shell), and the freshly created
        // inner navigator lands at depth 1 rather than stacking the leaf over
        // a placeholder root.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.stacks().0, vec!["/".to_string()]);

        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(
            h.stacks().0,
            vec!["/".to_string(), "/sessions".to_string()],
            "the shell page stacked over the root page, which is retained"
        );
        assert_eq!(
            h.inner.depth(),
            1,
            "the fresh inner navigator is at its root"
        );
        assert_eq!(h.stacks().1, vec!["/sessions".to_string()]);
    }

    #[test]
    fn a_page_pushed_over_the_shell_still_counts_as_placed() {
        // The shell page is retained BELOW an outer push, so a navigation into
        // its subtree must not place a second one (two live navigators on one
        // controller); it lands under the covering page instead.
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();
        h.outer.push(|| sized(77.0, 77.0));
        h.rebuild();
        assert_eq!(h.outer.depth(), 2);

        h.router.go("/terminal");
        h.rebuild();
        assert_eq!(h.outer.depth(), 2, "no second shell page was placed");
        assert_eq!(h.shell_builds.get(), 1);
        assert_eq!(h.stacks().1, vec!["/terminal".to_string()]);
    }

    #[test]
    fn two_shell_navigations_in_one_frame_place_the_shell_page_once() {
        // The intent-vs-fact gap: the second navigation resolves before any
        // rebuild published the first one's placement, so the committed stack
        // alone cannot answer the keep rule.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.push("/sessions");
        h.router.push("/terminal");
        h.rebuild();

        assert_eq!(
            h.stacks().0,
            vec!["/".to_string(), "/sessions".to_string()],
            "exactly one shell page, placed by the first navigation"
        );
        assert_eq!(
            h.stacks().1,
            vec!["/sessions".to_string(), "/terminal".to_string()],
            "both in-shell ops landed on the one inner navigator"
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn the_root_route_beats_a_pathless_shell_for_the_empty_location() {
        // A zero-consuming shell also matches the empty segment list, so `/`
        // must be declared before it — `match_routes` takes the first match.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.stacks().0, vec!["/".to_string()]);
        assert!(
            !h.inner.is_mounted(),
            "the shell was never placed, so its navigator never mounted"
        );
    }

    #[test]
    fn a_redirect_out_of_the_shell_splits_the_post_redirect_chain() {
        // A guard bouncing a child out of its shell resolves BEFORE the split,
        // so the shell page is never placed at all.
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let router = {
            let inner_for_shell = inner.clone();
            Router::with_controller(
                &outer,
                vec![
                    Route::new("/", |_| sized(10.0, 10.0)),
                    shell_route(
                        &inner,
                        move |_| {
                            let inner = inner_for_shell.clone();
                            any(navigator(&inner, || sized(11.0, 11.0)))
                        },
                        vec![
                            Route::new("/sessions", |_| sized(20.0, 20.0))
                                .redirect(|_| Some("/".to_string())),
                        ],
                    ),
                ],
            )
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = outer.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());

        router.go("/sessions");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&outer), vec!["/".to_string()], "the redirect won");
        assert!(!inner.is_mounted(), "no shell page, no inner navigator");
    }

    #[test]
    fn a_non_shell_chain_still_flattens_onto_one_controller() {
        // The no-regression half: a nested chain with no shell binding lands
        // entirely on the router's own controller, exactly as before.
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(
            &controller,
            vec![
                Route::new("/home", |_| sized(10.0, 10.0)),
                Route::new("/users", |_| sized(20.0, 20.0))
                    .child(Route::new(":id", |_| sized(30.0, 30.0))),
            ],
        );
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = controller.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());

        router.go("/users/42");
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            paths(&controller),
            vec!["/users/42".to_string(), "/users/42".to_string()],
            "go replaced the top with the chain root and pushed the rest — both \
             pages on the one controller"
        );

        router.push("/users/7");
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            controller.depth(),
            3,
            "push stacked the leaf alone, unchanged"
        );

        router.replace("/home");
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            paths(&controller).last().map(String::as_str),
            Some("/home"),
            "replace swapped the top alone, unchanged"
        );
        assert_eq!(controller.depth(), 3);
    }
}
