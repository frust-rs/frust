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
//! [`go`](Router::go) makes the stack exactly the matched chain, as go_router's
//! `go` does: a page whose key is in the new chain keeps its state, every other
//! page leaves. [`push`](Router::push) stacks the matched leaf (the page below is
//! retained); [`replace`](Router::replace) swaps the top for the matched leaf;
//! [`pop`](Router::pop) pops one page.
//! [`go_named`](Router::go_named)/[`push_named`](Router::push_named) resolve a
//! named route's params into a path first. Every one drives the owned
//! [`NavigatorController`] — no reactive types here; the signal glue that feeds
//! [`handle_location`](Router::handle_location) deep links lives in the facade,
//! keeping `frust-widgets` reactive-free.
//!
//! The router is plain data + logic an app keeps in its `Component::State`
//! alongside the controller — no global registry.
//!
//! # What a go does
//!
//! * **Keys.** A page is keyed by the concrete path matched up to its route, query
//!   excluded, and a shell page by its inner controller. A chain page whose key is
//!   on the stack retains that page: its widget state, result callback and back
//!   policy stay, and it takes the new params and stamp. Pages placed by
//!   [`push`](Router::push), [`replace`](Router::replace) or a bare
//!   [`NavigatorController`] op have no key and never match, except shell pages.
//! * **Identity belongs in the path.** A go to the current path keeps the page's
//!   state and hands it the new query, and a same-path link does the same. So a
//!   value that identifies what a page shows goes in the path: as a query param it
//!   would change under state retained from the old value.
//! * **Removed pages.** Every other page leaves, route-less overlays included. Each
//!   one's result callback fires once with an empty
//!   [`PopResult`](super::options::PopResult), on the go rather than when any
//!   animation settles. Routes inside a nested navigator that is disposed with its
//!   host do not fire.
//! * **Gates belong in a redirect.** Any go, one from a link included, closes an
//!   overlay whose [`BackPolicy::Veto`](super::options::BackPolicy::Veto) refuses
//!   a back press. A security gate must be a route redirect, not an overlay.
//! * **Transition.** A fresh top page pushes in with its route's
//!   [`Route::transition`] or the navigator's default, over the leaving top. If the
//!   top was removed and the new top was retained, the old top pops with its own
//!   transition reversed. If the top is unchanged nothing animates, and pages
//!   removed below it leave silently. A first-build navigator never animates, so a
//!   freshly placed shell or a cold-start link appears at once.
//! * **Timing.** Depth, back interest and the route stack report the new stack on
//!   the rebuild that applies the go, even while the flight runs. Within one frame
//!   the last go wins outright: a page an earlier navigation queued is never built,
//!   unless it was pushed with a result callback, and a page on the stack that the
//!   last go names keeps its state.
//! * **Stamps.** Each page publishes a location on
//!   [`NavigatorController::route_stack`] (`R-B1`), so no consumer re-derives the
//!   page-to-route mapping. The chain's leaf publishes the whole location, an
//!   ordinary parent its own matched path with no query, so `/users/42?tab=a` reads
//!   `[/users, /users/42?tab=a]`. A shell page publishes the whole location that
//!   placed it and keeps that across a go, because the keep rule re-matches it
//!   through the shell (see [`shell_route`]).
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
//! The **keep rule** is the point: a navigation *within* a placed shell must
//! not replace its page, because that would drop the retained inner navigator
//! and every page in it, which is the whole thing a shell exists to prevent.
//! [`go`](Router::go) keeps the page by key: a shell page's key is its inner
//! controller, so it survives any go whose chain crosses it. [`push`](Router::push)
//! and [`replace`](Router::replace) issue **zero** ops on the enclosing stack when
//! the shell is already placed. "Already placed?" is answered by the published
//! route stack ([`NavigatorController::route_stack`]) — fact, not the op queue's
//! intent — after this router's own record of what it queued in this same frame
//! (see [`shell_route`] for the full table and the staleness note).
//!
//! # Deferred (documented seams)
//!
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

use super::controller::erase_page;
use super::navigator::{NavigatorController, NavigatorId, PushOptions, ReplaceOptions};
use super::options::{PageKey, PageSpec};
use super::path::{Location, PathPattern, RouteParams, encode_segment};
use super::route::{NavRequest, RouteNavigator};
use super::transition::TransitionSpec;

/// Report an ignored caller option in debug builds; a no-op in release.
#[cfg_attr(not(debug_assertions), allow(unused_variables))]
fn log_ignored(call: &str, why: &str) {
    #[cfg(debug_assertions)]
    eprintln!("frust-widgets: Router::{call} options ignored: {why}");
}

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
    /// This route's own page transition (see [`Route::transition`]).
    transition: Option<TransitionSpec>,
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
            transition: None,
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

    /// Give this route's page its own transition. Whenever the router pushes
    /// or replaces a page for this route (`go`, `push`, `replace`, named
    /// variants, deep links, and requests queued through a
    /// [`RouteNavigator`]), the page carries `spec` instead of the navigator's
    /// default transition. In a nested chain each pushed page uses its own
    /// route's spec. A route without one keeps the navigator default.
    ///
    /// A `go` animates at most one page: a fresh top page pushes in with its own
    /// spec, whatever the specs of the pages below it say.
    ///
    /// Precedence, highest first: the options given to
    /// [`push_with`](Router::push_with), this route-level spec, the
    /// navigator's default. Reduce motion collapses it like any other
    /// transition.
    pub fn transition(mut self, spec: TransitionSpec) -> Self {
        self.transition = Some(spec);
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
/// The resolved chain splits at the boundary, and each controller gets its own
/// segment. A placed shell page must survive, because replacing it would drop the
/// retained inner navigator and every page in it (the **keep rule**):
///
/// | Verb | Enclosing controller | Inner controller |
/// |---|---|---|
/// | [`go`](Router::go) | the stack becomes exactly the segment; a placed shell page is **retained by key** | the stack becomes exactly the segment (a shell matched as the leaf has none, and keeps what it shows) |
/// | [`push`](Router::push) | shell already placed → **keep** (zero ops); else *push* the shell page (the page below — a `/` connect screen, say — is retained) | push the leaf |
/// | [`replace`](Router::replace) | shell already placed → **keep** (zero ops); else replace the top with the shell page and push the rest | replace the top with the leaf |
/// | [`pop`](Router::pop) | pops the router's own controller — see the [module docs](self)' deferred note | — |
///
/// One structural op per controller per navigation, so nothing here rests on
/// stacking two transitions in one `apply_ops` pass.
///
/// **A go keeps exactly what its chain names.** A shell survives a go because its
/// key is in the new list, and a page below it survives only if the chain names it
/// too. To keep a page below a shell, nest the shell under it in the route tree,
/// which makes that page part of every chain through the shell:
///
/// ```ignore
/// Router::with_controller(&outer, vec![
///     Route::new("/", connect_page).child(shell_route(&inner, shell_page, vec![
///         Route::new("/sessions", sessions_page),
///         Route::new("/terminal", terminal_page),
///     ])),
/// ])
/// ```
///
/// Here `go("/terminal")` from anywhere leaves `[/, shell]` outside and
/// `[terminal]` inside, and a back press at inner depth 1 pops to `/`. With `/` and
/// the shell declared side by side, as in the first example above, the same go
/// leaves `[shell]` outside, and a back press leaves the app. The page below the
/// shell keeps its state only if a go placed it, since a pushed page has no key
/// and is rebuilt.
///
/// **A placement resets the verb below it.** When a push or a replace has to
/// place the shell page, the inner navigator is brand new — its stack is just the
/// root page its `navigator(…)` call names — so there is no in-shell history to
/// stack onto and every segment below the placement replaces that root page
/// whatever the caller asked for. That is what makes `push("/sessions")` from a
/// bar-less `/` land as `[/, shell]` outside and `[sessions]` (depth 1) inside,
/// so a back press at `/sessions` leaves the shell instead of popping to a
/// placeholder root.
///
/// [`push_with`](Router::push_with) is the exception: its caller's options
/// (result callback, back policy, barrier, transition) can only ride on a pushed
/// page, so a leaf placed with a fresh shell is pushed on top of the inner
/// navigator's root page instead, and popping it returns to that root. So back
/// from that leaf lands on the shell's root page before leaving the shell,
/// where `push(…)` to the same location places the leaf as the root.
///
/// # Matched state reaching the shell page
///
/// `builder` receives the resolved chain's merged [`RouteParams`] like any
/// other route — but only as of the last navigation that reached it: the one that
/// *placed* it, or a later [`go`](Router::go) through it. A push or a replace
/// within the shell issues no op on the enclosing controller, so it does not
/// reach the shell page at all. Whatever reaches it, the page's published route
/// entry keeps naming the location that placed it, so a go within the shell
/// changes nothing on the enclosing stack. Live
/// in-shell state is read from the **inner** navigator instead — its own
/// [`route_stack`](NavigatorController::route_stack) (or the facade's
/// `RouteObserver` over it), which is authoritative at every publish. Chrome
/// inside the shell page (a title bar) reads that, never the enclosing stack.
///
/// # Deep links landing mid-shell
///
/// A `go` that runs before the shell page exists — a cold-start deep link
/// resolved from `Component::init` — queues the inner segment's op on `inner`
/// while it has no widget at all. It is drained by the inner navigator's
/// *first* `build` (the same pre-first-frame drain a top-level controller
/// gets), so the shell's first painted frame is already the linked child: no
/// flash, no second navigation. If a shell page defers mounting its navigator
/// (rendering a loading state first), the op simply waits on the queue and lands
/// on whichever build mounts it, unless a later go leaves that shell first and
/// discards it.
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
        transition: None,
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
    /// What this page publishes on the route stack (see the [module docs](self)'
    /// *Stamps*): [`location`](Self::location) for a chain's leaf or a shell page,
    /// otherwise this page's own matched path, so a parent never reads as its child.
    stamp: Location,
    /// The route's own [`Route::transition`], if it set one.
    transition: Option<TransitionSpec>,
    /// The [`shell_route`] binding of the route this page came from: the
    /// controller every page *below* it in the chain applies to. `None` for
    /// every ordinary route, which is what keeps a non-shell chain flattening
    /// onto one controller.
    shell: Option<NavigatorController<State>>,
    /// What a [`go`](Router::go) retains this page by: the concrete path matched
    /// up to this route with the query excluded (go_router's matched path), or
    /// for a shell page its inner controller.
    key: PageKey,
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

    /// The push options for this page: opaque, carrying its stamp and the route's
    /// own transition when it set one. A shell page also carries its key, so a
    /// later [`go`](Router::go) keeps it; every other page placed by a push is
    /// unkeyed, and a go never matches it.
    fn push_options(&self) -> PushOptions<State> {
        let mut options = PushOptions::opaque().route(self.stamp.clone());
        if let Some(spec) = self.transition {
            options = options.transition(spec);
        }
        if self.shell.is_some() {
            options = options.key(self.key.clone());
        }
        options
    }

    /// The replace options for this page (see [`push_options`](Self::push_options)).
    fn replace_options(&self) -> ReplaceOptions {
        let mut options = ReplaceOptions::opaque().route(self.stamp.clone());
        if let Some(spec) = self.transition {
            options = options.transition(spec);
        }
        if self.shell.is_some() {
            options = options.key(self.key.clone());
        }
        options
    }

    /// This page as one entry of a [`go`](Router::go)'s list: opaque, keyed, and
    /// carrying its stamp and the route's own transition when it set one.
    fn into_page_spec(self) -> PageSpec<State> {
        let key = self.key.clone();
        let transition = self.transition;
        let route = self.stamp.clone();
        PageSpec {
            key: Some(key),
            builder: erase_page(self.into_page_builder()),
            opaque: true,
            transition,
            route: Some(route),
        }
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

/// Which navigation verb a resolved chain is being applied under, and what lets
/// `push` and `replace` share one chain-splitting path. A `go` is not a verb here:
/// it sets each controller's page list instead (see [`Router::go`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    Push,
    Replace,
    /// What every segment below a fresh shell placement applies as, whichever
    /// verb the caller used: the first page replaces the top, the rest push.
    Place,
}

/// What a [`go`](Router::go) queued on one controller, until that controller
/// applies it (see [`Router::queued_gos`]).
struct QueuedGo {
    /// The controller's [`route_generation`](NavigatorController::route_generation)
    /// when the go queued.
    generation: u64,
    /// The inner-controller id of the shell page the queued list ends with, `None`
    /// when it has none.
    shell: Option<NavigatorId>,
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
    /// Shell pages a push or a replace **queued** but whose target controller has
    /// not published them yet, keyed by the shell's inner-controller id and
    /// valued by that target's [`route_generation`](NavigatorController::route_generation)
    /// at queue time.
    ///
    /// The keep rule reads committed state (the published route stack), which
    /// is the right authority — but it leaves one gap the router alone can
    /// close: two shell-crossing navigations applied in the *same* frame, before
    /// any rebuild, would each see a stack without the shell page and each queue
    /// one, leaving two shell pages driving a single inner controller. An entry
    /// is honoured only while [`queued_note_is_live`](Self::queued_note_is_live),
    /// so it self-invalidates on the very pass that makes the stack authoritative —
    /// it is a note about this router's own queued intent, never a second copy
    /// of the stack. A go clears every entry: its record below answers instead.
    pending_shells: RefCell<HashMap<NavigatorId, u64>>,
    /// The last [`go`](Self::go) queued per controller, keyed by that controller's
    /// id. The same kind of note as [`pending_shells`](Self::pending_shells): a go
    /// makes the stack exactly its list, so until it is applied the record, not
    /// the committed stack, says which shell the stack holds. That is what lets a
    /// push in the same frame answer from the go, in both directions.
    queued_gos: RefCell<HashMap<NavigatorId, QueuedGo>>,
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
            queued_gos: RefCell::new(HashMap::new()),
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
            for (route, _) in &chain {
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
            let leaf = chain.len() - 1;
            let pages = chain
                .iter()
                .enumerate()
                .map(|(index, (route, matched))| ResolvedPage {
                    builder: route.builder.clone(),
                    params: merged.clone(),
                    location: loc.clone(),
                    stamp: if index == leaf || route.shell.is_some() {
                        loc.clone()
                    } else {
                        matched_location(&loc.segments[..*matched])
                    },
                    transition: route.transition,
                    shell: route.shell.clone(),
                    key: match &route.shell {
                        Some(inner) => PageKey::Shell(inner.id()),
                        None => PageKey::Path(matched_path(&loc.segments[..*matched])),
                    },
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
    fn match_chain<'a>(&'a self, loc: &Location) -> Option<(MatchedChain<'a, State>, RouteParams)> {
        match_routes(
            &self.routes,
            &loc.segments,
            0,
            RouteParams::new(),
            Vec::new(),
        )
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

    /// Make the stack exactly the matched chain, as go_router's `go` does: a page
    /// whose key is in the chain stays with its state and takes the new params, and
    /// every other page leaves, route-less overlays included. An unmatched location
    /// makes the stack one error page.
    ///
    /// Across a [`shell_route`] each controller's stack becomes its own segment, and
    /// anything queued on a shell the chain sets no pages on is discarded. Depth,
    /// back interest and the route stack report the result on the rebuild that
    /// applies the go, and within one frame the last go wins.
    ///
    /// See the [module docs](self)' *What a go does*, which also says why identifying
    /// values belong in the path and a security gate in a route redirect.
    pub fn go(&self, location: &str) {
        match self.resolved(location) {
            Resolution::Matched { pages, .. } => self.go_chain(self.split_chain(pages)),
            Resolution::Error { location } => self.go_error(location),
        }
    }

    /// Push the matched leaf page onto the stack (the page below is retained).
    ///
    /// Stamps the leaf's [`ResolvedPage::location`] — see [`go`](Self::go)'s doc.
    /// Across a [`shell_route`] boundary the leaf lands on the shell's inner
    /// controller — see [`shell_route`]'s table.
    pub fn push(&self, location: &str) {
        self.apply(self.resolved(location), Verb::Push, None);
    }

    /// [`push`](Self::push) with caller-supplied [`PushOptions`] for the
    /// pushed leaf page: a per-push transition, back policy, result callback,
    /// and so on. Resolution and redirects are exactly `push`'s.
    ///
    /// - The router stamps the page's resolved location, replacing any
    ///   `.route(..)` set on `options`.
    /// - Transition precedence, highest first: `options`' transition, the
    ///   route's [`Route::transition`], the navigator's default.
    /// - `options` apply to the leaf page only; shell or parent pages pushed
    ///   by the same request use their own route's spec.
    /// - Across a [`shell_route`] boundary the options still apply to the
    ///   matched leaf. If the shell page has to be placed first, the leaf is
    ///   pushed on top of the root page the shell's inner navigator names (so
    ///   `on_result`, the back policy and the rest all take effect, and a pop
    ///   returns to that root page); if the matched leaf is the shell route
    ///   itself, the options apply to the shell page. When the shell page is
    ///   already on the stack and the shell route is the leaf, no page is
    ///   pushed and the options are ignored.
    /// - Back from a leaf pushed into a freshly placed shell lands on the
    ///   shell navigator's root page before leaving the shell, whereas
    ///   [`push`](Self::push) to the same location (no options) places the
    ///   leaf as the root, so one back press leaves the shell.
    /// - A location that resolves to the error page pushes it exactly as
    ///   `push` does and ignores `options`.
    /// - This is a `Router` method only: a [`RouteNavigator`] queues plain
    ///   data requests that cannot carry `PushOptions` (closures), so its
    ///   pushes get the route-level transition but no per-push options.
    pub fn push_with(&self, location: &str, options: PushOptions<State>) {
        self.apply(self.resolved(location), Verb::Push, Some(options));
    }

    /// Replace the top page with the matched leaf (the top's state is dropped;
    /// the stack depth is unchanged) — the op a [`NavRequest::Replace`] applies.
    ///
    /// Stamps the leaf's [`ResolvedPage::location`] — see [`go`](Self::go)'s doc.
    /// Across a [`shell_route`] boundary the replaced top is the shell's inner
    /// one — see [`shell_route`]'s table.
    pub fn replace(&self, location: &str) {
        self.apply(self.resolved(location), Verb::Replace, None);
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
    fn apply(&self, resolution: Resolution<State>, verb: Verb, leaf: Option<PushOptions<State>>) {
        match resolution {
            Resolution::Matched { pages, .. } => {
                let mut segments = self.split_chain(pages);
                if segments.len() == 1 {
                    let segment = segments.pop().expect("a chain has at least one segment");
                    Self::apply_flat(&segment.controller, segment.pages, verb, leaf);
                } else {
                    self.apply_shell_chain(segments, verb, leaf);
                }
            }
            Resolution::Error { location } => match verb {
                Verb::Push => self.controller.push(self.error_page_builder(location)),
                Verb::Replace | Verb::Place => {
                    self.controller.replace(self.error_page_builder(location))
                }
            },
        }
    }

    /// Queue a [`go`](Self::go)'s pages: each segment's controller gets exactly its
    /// segment. A trailing empty segment (a shell matched as the leaf) issues no
    /// inner op and its queue is forgotten with the rest, so the shell keeps what it
    /// shows: an earlier go still queued there cannot decide the inner stack.
    fn go_chain(&self, segments: Vec<Segment<State>>) {
        let targets = segments
            .iter()
            .filter(|s| !s.pages.is_empty())
            .map(|s| s.controller.id())
            .collect();
        self.forget_queued_outside(&targets);
        for segment in segments {
            if segment.pages.is_empty() {
                continue;
            }
            let shell = segment
                .pages
                .last()
                .and_then(|page| page.shell.as_ref())
                .map(NavigatorController::id);
            let pages = segment
                .pages
                .into_iter()
                .map(ResolvedPage::into_page_spec)
                .collect();
            self.queue_go(&segment.controller, shell, pages);
        }
    }

    /// Queue a [`go`](Self::go) that makes the stack one error page for `location`.
    /// Unstamped, like every other error page; keyed by location, so a repeated go
    /// to one bad location keeps the page.
    fn go_error(&self, location: Location) {
        self.forget_queued_outside(&HashSet::from([self.controller.id()]));
        let key = PageKey::Path(format!("{}(error)", location.location_string()));
        let page = PageSpec {
            key: Some(key),
            builder: erase_page(self.error_page_builder(location)),
            opaque: true,
            transition: None,
            route: None,
        };
        self.queue_go(&self.controller, None, vec![page]);
    }

    /// Record and queue one controller's page list.
    fn queue_go(
        &self,
        controller: &NavigatorController<State>,
        shell: Option<NavigatorId>,
        pages: Vec<PageSpec<State>>,
    ) {
        let record = QueuedGo {
            generation: controller.route_generation(),
            shell,
        };
        self.queued_gos.borrow_mut().insert(controller.id(), record);
        controller.set_pages(pages);
    }

    /// Forget everything queued that a go setting the `targets` controllers' stacks
    /// makes stale: every shell placement noted, because the go decides afresh what
    /// each stack holds, and the queue of every shell it sets no pages on, whose ops
    /// would otherwise land on that shell's next mount or undo what it keeps showing.
    fn forget_queued_outside(&self, targets: &HashSet<NavigatorId>) {
        self.pending_shells.borrow_mut().clear();
        visit_shells(&self.routes, &mut |shell| {
            if !targets.contains(&shell.id()) {
                shell.discard_queued();
                self.queued_gos.borrow_mut().remove(&shell.id());
            }
        });
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
    fn apply_shell_chain(
        &self,
        segments: Vec<Segment<State>>,
        verb: Verb,
        leaf: Option<PushOptions<State>>,
    ) {
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
        // onto, so it applies as `Place` (see `shell_route`'s "a placement resets
        // the verb below it").
        let mut enclosing_kept = true;
        let mut leaf = leaf;
        // A shell route matched as the leaf has no inner page: the caller's
        // options belong to the shell page itself.
        let shell_is_leaf = segments[last].pages.is_empty();

        for (index, segment) in segments.into_iter().enumerate() {
            let effective = if enclosing_kept { verb } else { Verb::Place };
            if index == last {
                if segment.pages.is_empty() {
                    if leaf.is_some() {
                        // The shell page was already placed, so no op carries
                        // the caller's options.
                        log_ignored(
                            "push_with",
                            "the matched shell page is already on the stack, so its options do not apply",
                        );
                    }
                } else if !enclosing_kept && leaf.is_some() {
                    Self::push_onto_shell_root(&segment.controller, segment.pages, leaf.take());
                } else {
                    Self::apply_flat(&segment.controller, segment.pages, effective, leaf.take());
                }
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
            let shell_options = if shell_is_leaf && index + 1 == last {
                leaf.take()
            } else {
                None
            };
            Self::place_segment(&segment.controller, segment.pages, effective, shell_options);
        }
    }

    /// Push a freshly placed shell's inner chain on top of the root page its
    /// navigator names, with the caller's options on the leaf.
    ///
    /// A root page can only be replaced, and a replace carries no result
    /// callback, back policy, barrier, dismiss signal, visibility callback or
    /// edge-swipe override. Pushing keeps every field of the caller's options
    /// in force, at the price of one extra page beneath the leaf.
    fn push_onto_shell_root(
        controller: &NavigatorController<State>,
        pages: Vec<ResolvedPage<State>>,
        leaf_options: Option<PushOptions<State>>,
    ) {
        Self::push_chain(controller, pages.into_iter(), leaf_options);
    }

    /// Push `pages` in order onto `controller`: earlier pages with their own
    /// route's options, the last with the caller's (when given) over its own.
    fn push_chain(
        controller: &NavigatorController<State>,
        pages: impl Iterator<Item = ResolvedPage<State>>,
        leaf_options: Option<PushOptions<State>>,
    ) {
        let mut pages = pages.peekable();
        let mut leaf_options = leaf_options;
        while let Some(page) = pages.next() {
            let options = if pages.peek().is_none() {
                Self::leaf_push_options(&page, leaf_options.take())
            } else {
                page.push_options()
            };
            controller.push_with_options(page.into_page_builder(), options);
        }
    }

    /// The keep rule's question for a push or a replace: is the page of the shell
    /// that opens boundary `boundary` already on `target`'s stack?
    ///
    /// This router's own queued intent answers first: a placement queued since the
    /// last go, then that go's list, which is exactly what the stack will hold.
    /// Otherwise it is answered against the **published** route stack — the
    /// committed fact, per `route_state`'s intent-vs-fact split — by re-matching
    /// each entry's
    /// location through the route table and asking whether that chain crosses
    /// the same shell at the same boundary. Re-matching (rather than comparing
    /// locations) is what keeps a pathless shell honest: the shell page is
    /// stamped with the location that placed it, and every one of its children
    /// resolves through the same shell. A parent above the shell publishes its own
    /// path, whose chain stops short of the shell, so it never reads as placing it.
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
        if let Some(&generation) = self.pending_shells.borrow().get(&shell)
            && Self::queued_note_is_live(target, generation)
        {
            return true;
        }
        if let Some(go) = self.queued_gos.borrow().get(&target.id())
            && Self::queued_note_is_live(target, go.generation)
        {
            return go.shell == Some(shell);
        }
        target
            .route_stack()
            .entries()
            .iter()
            .flatten()
            .any(|location| self.shell_at(location, boundary) == Some(shell))
    }

    /// Whether a note taken at `generation` about ops queued on `target` still
    /// speaks for them, rather than the published stack.
    ///
    /// A publish ends a note, but a generation alone cannot: a batch that leaves
    /// the stack identical publishes nothing, and a note outliving it would answer
    /// for a stack long since changed. So a note also ends when the queue it was
    /// about has drained. An interactive swipe in flight is the exception, because
    /// it holds back the publish of a batch it has already drained (see
    /// `route_state`'s staleness contract).
    fn queued_note_is_live(target: &NavigatorController<State>, generation: u64) -> bool {
        target.route_generation() == generation
            && (target.has_queued() || target.transition().interactive)
    }

    /// The inner-controller id of the `boundary`-th shell the chain matching
    /// `location` crosses, or `None` if it crosses fewer.
    fn shell_at(&self, location: &Location, boundary: usize) -> Option<NavigatorId> {
        let (chain, _) = self.match_chain(location)?;
        chain
            .iter()
            .filter_map(|(route, _)| route.shell.as_ref())
            .nth(boundary)
            .map(|controller| controller.id())
    }

    /// Apply one segment's pages to `controller` with the flat, single-controller
    /// semantics `push` and `replace` have always had — the behaviour a chain
    /// crossing no shell keeps byte-for-byte.
    fn apply_flat(
        controller: &NavigatorController<State>,
        pages: Vec<ResolvedPage<State>>,
        verb: Verb,
        leaf_options: Option<PushOptions<State>>,
    ) {
        match verb {
            Verb::Place => {
                let mut pages = pages.into_iter();
                if let Some(first) = pages.next() {
                    let options = first.replace_options();
                    controller.replace_with_options(first.into_page_builder(), options);
                }
                Self::push_chain(controller, pages, leaf_options);
            }
            Verb::Push => {
                if let Some(leaf) = pages.into_iter().next_back() {
                    let options = Self::leaf_push_options(&leaf, leaf_options);
                    controller.push_with_options(leaf.into_page_builder(), options);
                }
            }
            Verb::Replace => {
                if let Some(leaf) = pages.into_iter().next_back() {
                    let options = leaf.replace_options();
                    controller.replace_with_options(leaf.into_page_builder(), options);
                }
            }
        }
    }

    /// Place a shell-carrying segment (its pages end with the shell page) on
    /// `controller`. A `push` stacks the whole segment so the page it came from
    /// is retained beneath the shell; otherwise the first page replaces the top
    /// and the rest push.
    ///
    /// `shell_options` are the caller's options when the shell page is itself
    /// the matched leaf; they are applied to the shell page, never dropped.
    fn place_segment(
        controller: &NavigatorController<State>,
        pages: Vec<ResolvedPage<State>>,
        verb: Verb,
        shell_options: Option<PushOptions<State>>,
    ) {
        match verb {
            Verb::Push => Self::push_chain(controller, pages.into_iter(), shell_options),
            // An enclosing fresh placement may have reset the verb. The caller's
            // options (if any) still belong to the shell page, which lands on
            // this controller's root by the same push-over-root rule an
            // ordinary leaf gets.
            Verb::Replace | Verb::Place => match shell_options {
                Some(_) => Self::push_onto_shell_root(controller, pages, shell_options),
                None => Self::apply_flat(controller, pages, Verb::Place, None),
            },
        }
    }

    /// The options for the pushed leaf: the caller's, when given, with the
    /// page's location stamped and the route's transition filled in if the
    /// caller set none; otherwise the page's own.
    fn leaf_push_options(
        page: &ResolvedPage<State>,
        caller: Option<PushOptions<State>>,
    ) -> PushOptions<State> {
        match caller {
            None => page.push_options(),
            Some(mut options) => {
                options = options.route(page.stamp.clone());
                if options.transition.is_none() {
                    options.transition = page.transition;
                }
                if page.shell.is_some() {
                    options = options.key(page.key.clone());
                }
                options
            }
        }
    }

    /// The single entry point a deep link resolves through: a [`go`](Self::go) for a
    /// matched location, so a link arriving while the app runs replaces the stack
    /// with the link's chain, as go_router does for a platform link. A flow that
    /// must not lose its stack handles the link itself.
    ///
    /// An unmatched location pushes the error page instead, and back returns from
    /// it. This differs from go_router on purpose: a stray or unrelated link cannot
    /// wipe the stack, while a [`go`](Self::go) from app code still replaces it. The
    /// facade tracks the deep-link signal and calls this; this crate stays
    /// reactive-free.
    pub fn handle_location(&self, location: &str) {
        match self.resolved(location) {
            Resolution::Matched { pages, .. } => self.go_chain(self.split_chain(pages)),
            Resolution::Error { location } => {
                self.controller.push(self.error_page_builder(location));
            }
        }
    }

    /// Resolve `name` + `params` into a path, then [`go`](Self::go) to it. An
    /// unknown name makes the stack one error page.
    pub fn go_named(&self, name: &str, params: &RouteParams) {
        match self.path_for_name(name, params) {
            Some(path) => self.go(&path),
            None => {
                let location = named_error_location(name);
                self.route_nav.set_location(location.clone());
                self.go_error(location);
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

/// A matched route chain, root→leaf: each route with how many of the location's
/// segments the chain has consumed up to and including it.
type MatchedChain<'a, State> = Vec<(&'a Route<State>, usize)>;

/// Recursively match `routes` against the remaining location `segments`,
/// accumulating params and the visited route chain. `matched` counts the segments
/// the chain has already consumed. A route matches as a *prefix*; if it consumes
/// all remaining segments it is the chain leaf, otherwise the match recurses into
/// its children with the remainder.
fn match_routes<'a, State: 'static>(
    routes: &'a [Route<State>],
    segments: &[String],
    matched: usize,
    acc_params: RouteParams,
    chain: MatchedChain<'a, State>,
) -> Option<(MatchedChain<'a, State>, RouteParams)> {
    for route in routes {
        let pattern = PathPattern::parse(&route.path);
        let Some((consumed, params)) = pattern.match_prefix(segments) else {
            continue;
        };

        let mut next_params = acc_params.clone();
        next_params.extend(params);

        let matched = matched + consumed;
        let mut next_chain = chain.clone();
        next_chain.push((route, matched));

        let remaining = &segments[consumed..];
        if remaining.is_empty() {
            return Some((next_chain, next_params));
        }
        // More segments left: only a child match can consume them.
        if let Some(found) =
            match_routes(&route.children, remaining, matched, next_params, next_chain)
        {
            return Some(found);
        }
        // This route matched a prefix but no child covered the rest — keep trying
        // sibling routes.
    }
    None
}

/// The concrete path `segments` spell, re-encoded so a `/` inside a segment stays
/// distinct from a segment boundary. A [`go`](Router::go) retains a page by this.
fn matched_path(segments: &[String]) -> String {
    let encoded: Vec<String> = segments.iter().map(|s| encode_segment(s)).collect();
    format!("/{}", encoded.join("/"))
}

/// The location an ordinary parent page publishes: the path `segments` spell, the
/// same matched path its key names, with no query.
fn matched_location(segments: &[String]) -> Location {
    Location {
        path: format!("/{}", segments.join("/")),
        segments: segments.to_vec(),
        query: RouteParams::new(),
    }
}

/// Call `visit` with the inner controller of every [`shell_route`] in the tree.
fn visit_shells<State: 'static>(
    routes: &[Route<State>],
    visit: &mut impl FnMut(&NavigatorController<State>),
) {
    for route in routes {
        if let Some(inner) = &route.shell {
            visit(inner);
        }
        visit_shells(&route.children, visit);
    }
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
    use super::super::options::PopResult;
    use super::super::route_state::NavChange;
    use frust_theme::Theme;

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
    fn a_go_to_the_same_path_keeps_the_pages_state_and_takes_the_new_location() {
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

        // The navigator's own root page has no key, so the first go builds fresh.
        router.go("/home");
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);

        // Increment the home counter.
        root.event(&mut state, &down(5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1);

        // The same path retains the page, whatever the query says.
        router.go("/home?tab=posts");
        root.rebuild(&mut app, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
        assert_eq!(observed.get(), 1, "go kept the page's state");
        assert_eq!(
            controller.route_stack().entries(),
            &[Some(Location::parse("/home?tab=posts"))],
            "the retained page took the new location"
        );
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
            "the error page is the stack"
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
            "Router::go stamps the resolved location on the page list"
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

    #[test]
    fn a_parent_page_publishes_its_own_matched_path_and_the_leaf_the_whole_location() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(
            &controller,
            vec![
                Route::new("/users", |_| sized(10.0, 10.0)).child(
                    Route::new(":id", |_| sized(20.0, 20.0))
                        .child(Route::new("posts", |_| sized(30.0, 30.0))),
                ),
            ],
        );
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = controller.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());

        router.go("/users/42?tab=a");
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            controller.route_stack().entries(),
            &[
                Some(Location::parse("/users")),
                Some(Location::parse("/users/42?tab=a"))
            ],
            "a parent has no query, and the leaf keeps its own"
        );

        router.go("/users/a%2Fb/posts?tab=a");
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            controller.route_stack().entries(),
            &[
                Some(Location::parse("/users")),
                Some(Location::parse("/users/a%2Fb")),
                Some(Location::parse("/users/a%2Fb/posts?tab=a"))
            ],
            "the leaf of the last go is a parent now, and an encoded slash stays one segment"
        );

        // The public accessor still reports the location the chain resolved from.
        let Resolution::Matched { pages, .. } = router.resolve("/users/42?tab=a") else {
            panic!("expected a match");
        };
        assert_eq!(
            pages[0].location(),
            &Location::parse("/users/42?tab=a"),
            "location() is the raw resolved location for every page of the chain"
        );
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

    /// The shell page a [`ShellHarness`] table hands to [`shell_route`].
    type ShellPage = Box<dyn Fn(&RouteParams) -> AnyView<()>>;

    impl ShellHarness {
        fn new() -> Self {
            Self::with_routes(|inner, shell_page| {
                vec![
                    Route::new("/", |_| sized(10.0, 10.0)),
                    Route::new("/flat", |_| sized(40.0, 40.0)).transition(slide()),
                    shell_route(
                        inner,
                        shell_page,
                        vec![
                            Route::new("/sessions", |_| sized(20.0, 20.0)),
                            Route::new("/terminal", |_| sized(30.0, 30.0)),
                        ],
                    ),
                ]
            })
        }

        /// The route-tree shape: the shell nested under a `/` page, so every chain
        /// through the shell names that page too.
        fn under_root() -> Self {
            Self::with_routes(|inner, shell_page| {
                vec![
                    Route::new("/", |_| sized(10.0, 10.0)).child(shell_route(
                        inner,
                        shell_page,
                        vec![
                            Route::new("inbox", |_| sized(20.0, 20.0)),
                            Route::new("settings", |_| sized(30.0, 30.0)),
                        ],
                    )),
                    Route::new("/flat", |_| sized(40.0, 40.0)),
                ]
            })
        }

        fn with_routes(
            routes: impl FnOnce(&NavigatorController<()>, ShellPage) -> Vec<Route<()>>,
        ) -> Self {
            let outer: NavigatorController<()> = NavigatorController::new();
            let inner: NavigatorController<()> = NavigatorController::new();
            let shell_builds = Rc::new(Cell::new(0u32));

            let shell_page: ShellPage = {
                let inner = inner.clone();
                let builds = shell_builds.clone();
                Box::new(move |_: &RouteParams| {
                    let inner = inner.clone();
                    let builds = builds.clone();
                    any(crate::column()
                        .child(BuildCounter { builds })
                        .child(navigator(&inner, || sized(11.0, 11.0))))
                })
            };
            let router = Router::with_controller(&outer, routes(&inner, shell_page))
                .error_builder(|_| sized(99.0, 99.0));
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

    /// A harness whose `/` root is placed, with a result recorder.
    fn placed_root_harness() -> (ShellHarness, Rc<RefCell<Vec<PopResult>>>) {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        (h, Rc::new(RefCell::new(Vec::new())))
    }

    fn recording_options(results: &Rc<RefCell<Vec<PopResult>>>) -> PushOptions<()> {
        let results = results.clone();
        PushOptions::opaque().on_result(move |_, result| results.borrow_mut().push(result))
    }

    #[test]
    fn push_with_into_a_fresh_shell_keeps_the_caller_options_on_the_leaf() {
        let (mut h, results) = placed_root_harness();

        h.router.push_with("/sessions", recording_options(&results));
        h.rebuild();
        assert_eq!(
            h.stacks().0,
            vec!["/".to_string(), "/sessions".to_string()],
            "the shell page stacked over the root page"
        );
        assert_eq!(
            h.stacks().1,
            vec!["-".to_string(), "/sessions".to_string()],
            "the leaf is pushed over the shell's root page so its options apply \
             (a back press lands on that root page before leaving the shell)"
        );

        h.inner.pop();
        h.rebuild();
        h.rebuild();
        assert_eq!(
            results.borrow().len(),
            1,
            "on_result fires when the pushed leaf pops"
        );
        assert_eq!(h.inner.depth(), 1);
    }

    #[test]
    fn push_with_to_a_nested_shell_route_under_a_fresh_shell_applies_the_options() {
        // Shell B is a pathless child of shell A, so `push_with` to its path
        // matches B as the leaf under a freshly placed A: the chain splits into
        // [A page], [B page], [empty].
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner_a: NavigatorController<()> = NavigatorController::new();
        let inner_b: NavigatorController<()> = NavigatorController::new();
        let page_a = {
            let inner_a = inner_a.clone();
            move |_: &RouteParams| any(navigator(&inner_a, || sized(11.0, 11.0)))
        };
        let page_b = {
            let inner_b = inner_b.clone();
            move |_: &RouteParams| any(navigator(&inner_b, || sized(12.0, 12.0)))
        };
        // Shell B owns the path `/b`, so it is the chain's leaf.
        let mut shell_b = shell_route(
            &inner_b,
            page_b,
            vec![Route::new("/leaf", |_| sized(20.0, 20.0))],
        );
        shell_b.path = "/b".to_string();
        let routes = vec![shell_route(&inner_a, page_a, vec![shell_b])];
        let router = Router::with_controller(&outer, routes);
        let mut app = {
            let c = outer.clone();
            move |_: &mut ()| navigator(&c, || sized(10.0, 10.0))
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        let results = Rc::new(RefCell::new(Vec::new()));

        router.push_with("/b", recording_options(&results).transition(slide()));
        root.rebuild(&mut app, &mut ());
        // The inner ops drain when the freshly placed navigators first build.
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            paths(&outer),
            vec!["-".to_string(), "/b".to_string()],
            "the fresh outer shell page stacked over the seed page"
        );
        assert_eq!(
            paths(&inner_a),
            vec!["-".to_string(), "/b".to_string()],
            "the nested shell page is pushed over the outer shell's root page"
        );
        assert!(
            inner_a.transition().active,
            "the caller's transition runs on the nested shell page"
        );

        inner_a.pop();
        root.rebuild(&mut app, &mut ());
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            results.borrow().len(),
            1,
            "the caller's on_result is attached to the nested shell page"
        );
        assert_eq!(inner_a.depth(), 1);
    }

    #[test]
    fn push_with_into_a_fresh_shell_uses_the_caller_transition() {
        let animated = |spec: TransitionSpec| {
            let (mut h, _) = placed_root_harness();
            h.router
                .push_with("/sessions", PushOptions::opaque().transition(spec));
            h.rebuild();
            h.inner.transition().active
        };
        assert!(
            animated(slide()),
            "the caller's transition runs on the leaf"
        );
        assert!(!animated(TransitionSpec::NONE));
    }

    #[test]
    fn push_with_to_the_shell_route_applies_the_options_to_the_shell_page() {
        // Declared before `/`, a pathless shell matches the empty location
        // itself, so the shell page is the chain's leaf.
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let shell_page = {
            let inner = inner.clone();
            move |_: &RouteParams| any(navigator(&inner, || sized(11.0, 11.0)))
        };
        let routes = vec![
            shell_route(
                &inner,
                shell_page,
                vec![Route::new("/sessions", |_| sized(20.0, 20.0))],
            ),
            Route::new("/", |_| sized(10.0, 10.0)),
        ];
        let router = Router::with_controller(&outer, routes);
        let mut app = {
            let c = outer.clone();
            move |_: &mut ()| navigator(&c, || sized(10.0, 10.0))
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        let results = Rc::new(RefCell::new(Vec::new()));

        router.push_with("/", recording_options(&results));
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            outer.depth(),
            2,
            "the shell page stacked over the seed page"
        );

        outer.pop();
        root.rebuild(&mut app, &mut ());
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            results.borrow().len(),
            1,
            "the caller's on_result is attached to the shell page"
        );
    }

    #[test]
    fn push_with_into_a_placed_shell_still_uses_the_caller_options() {
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();
        let results = Rc::new(RefCell::new(Vec::new()));

        h.router.push_with("/terminal", recording_options(&results));
        h.rebuild();
        assert_eq!(h.outer.depth(), 1);
        assert_eq!(h.inner.depth(), 2);
        h.inner.pop();
        h.rebuild();
        h.rebuild();
        assert_eq!(results.borrow().len(), 1);
    }

    #[test]
    fn a_page_pushed_over_the_shell_still_counts_as_placed() {
        // The shell page is retained BELOW an outer push, so a push into its
        // subtree must not place a second one (two live navigators on one
        // controller); it lands under the covering page instead.
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();
        h.outer.push(|| sized(77.0, 77.0));
        h.rebuild();
        assert_eq!(h.outer.depth(), 2);

        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(h.outer.depth(), 2, "no second shell page was placed");
        assert_eq!(h.shell_builds.get(), 1);
        assert_eq!(
            h.stacks().1,
            vec!["/sessions".to_string(), "/terminal".to_string()]
        );
    }

    #[test]
    fn a_go_within_the_shell_closes_a_page_pushed_over_it() {
        // The stack is exactly the chain, and no chain names a route-less page.
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();
        h.outer.push(|| sized(77.0, 77.0));
        h.rebuild();
        assert_eq!(h.outer.depth(), 2);

        h.router.go("/terminal");
        h.rebuild();
        assert_eq!(h.stacks().0, vec!["/sessions".to_string()]);
        assert_eq!(h.shell_builds.get(), 1, "the shell underneath was retained");
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
            vec!["/users".to_string(), "/users/42".to_string()],
            "go made the stack the whole chain, both pages on the one controller"
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

    // ================= Go: the stack is exactly the chain =================

    /// Run frames from `start_ms` until nothing is in flight, returning the next
    /// free timestamp.
    fn settle(root: &mut RenderRoot<(), NavigatorView<()>>, app: &mut Build, start_ms: u64) -> u64 {
        let mut ms = start_ms;
        for _ in 0..10_000 {
            root.rebuild(app, &mut ());
            root.layout(Size::new(100.0, 100.0));
            let frame = FrameTime::from_nanos(ms * 1_000_000);
            let out = root.paint(&mut RecordingScene::default(), frame);
            ms += 16;
            if !out.needs_frame {
                return ms;
            }
        }
        panic!("the flight failed to settle");
    }

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn keys_are_concrete_matched_paths_without_the_query_and_shells_by_controller() {
        let inner: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::new(vec![
            Route::new("/", |_| sized(1.0, 1.0)),
            Route::new("/users", |_| sized(2.0, 2.0)).child(Route::new(":id", |_| sized(3.0, 3.0))),
            shell_route(
                &inner,
                |_| sized(4.0, 4.0),
                vec![Route::new("/inside", |_| sized(5.0, 5.0))],
            ),
        ]);
        let keys = |location: &str| -> Vec<PageKey> {
            let Resolution::Matched { pages, .. } = router.resolve(location) else {
                panic!("expected a match");
            };
            pages.iter().map(|page| page.key.clone()).collect()
        };
        let path = |path: &str| PageKey::Path(path.to_string());

        assert_eq!(keys("/"), vec![path("/")]);
        assert_eq!(
            keys("/users/42?tab=posts"),
            vec![path("/users"), path("/users/42")]
        );
        assert_eq!(
            keys("/inside"),
            vec![PageKey::Shell(inner.id()), path("/inside")]
        );
        assert_eq!(
            keys("/users/a%2Fb").last(),
            Some(&path("/users/a%2Fb")),
            "an encoded slash stays one segment"
        );
    }

    #[test]
    fn a_go_replaces_the_whole_stack_with_the_chain() {
        let mut h = PumpHarness::new(pump_routes());
        h.router.go("/home");
        let _ = h.frame();
        h.router.push("/detail");
        let _ = h.frame();
        assert_eq!(paths(h.router.controller()), strings(&["/home", "/detail"]));

        h.router.go("/other");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(30.0, 30.0))]);
        assert_eq!(paths(h.router.controller()), strings(&["/other"]));
        assert_eq!(h.router.controller().depth(), 1);
        assert!(!h.router.controller().can_pop());
        assert!(!h.router.controller().back_interest());
    }

    #[test]
    fn a_go_to_a_page_below_keeps_its_state_and_pops_the_top() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed = Rc::new(Cell::new(0u32));
        let router = {
            let obs = observed.clone();
            Router::with_controller(
                &controller,
                vec![
                    Route::new("/a", move |_| counter(&obs)),
                    Route::new("/b", |_| sized(20.0, 20.0)).transition(slide()),
                ],
            )
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = controller.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        router.go("/a");
        let ms = settle(&mut root, &mut app, 0);
        root.event(&mut (), &down(5.0, 5.0));
        let ms = settle(&mut root, &mut app, ms);
        assert_eq!(observed.get(), 1);
        router.push("/b");
        let ms = settle(&mut root, &mut app, ms);
        assert_eq!(paths(&controller), strings(&["/a", "/b"]));

        router.go("/a");
        root.rebuild(&mut app, &mut ());
        assert!(controller.transition().active, "the top animates out");
        assert!(controller.transition().is_pop);
        assert_eq!(controller.depth(), 1, "committed on the go's own frame");
        assert_eq!(controller.route_stack().change(), NavChange::Pop);

        settle(&mut root, &mut app, ms);
        assert_eq!(observed.get(), 1, "the retained page kept its state");
    }

    #[test]
    fn a_chain_go_places_two_pages_on_one_flight() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(
            &controller,
            vec![
                Route::new("/users", |_| sized(20.0, 20.0))
                    .transition(slide())
                    .child(Route::new(":id", |_| sized(30.0, 30.0)).transition(slide())),
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
        assert_eq!(paths(&controller), strings(&["/users", "/users/42"]));
        let flight = controller.transition();
        assert!(flight.active && !flight.is_pop);
        assert_eq!(
            (flight.from_depth, flight.to_depth),
            (1, 2),
            "one flight, from the old stack to the whole chain"
        );

        let ms = settle(&mut root, &mut app, 0);
        assert!(!controller.transition().active);
        assert_eq!(controller.depth(), 2);

        // A sibling child keeps the parent page and pushes a new top.
        router.go("/users/7");
        root.rebuild(&mut app, &mut ());
        assert!(controller.transition().active);
        assert_eq!(controller.depth(), 2);
        settle(&mut root, &mut app, ms);
    }

    #[test]
    fn a_page_removed_by_a_go_fires_its_result_callback_once_with_nothing() {
        let mut h = PumpHarness::new(pump_routes());
        let results = Rc::new(RefCell::new(Vec::new()));
        h.router.go("/home");
        let _ = h.frame();
        h.router.push_with("/detail", recording_options(&results));
        let _ = h.frame();

        h.router.go("/other");
        let _ = h.frame();
        let _ = h.frame();
        assert_eq!(results.borrow().len(), 1);
        assert!(results.borrow()[0].is_empty());
    }

    #[test]
    fn a_go_can_read_as_a_pop_or_publish_nothing() {
        let mut h = PumpHarness::new(pump_routes());
        h.router.go("/home");
        let _ = h.frame();
        h.router.push("/detail");
        let _ = h.frame();

        h.router.go("/home");
        let _ = h.frame();
        let controller = h.router.controller().clone();
        assert_eq!(controller.route_stack().change(), NavChange::Pop);
        let generation = controller.route_generation();

        h.router.go("/home");
        let _ = h.frame();
        assert_eq!(
            controller.route_generation(),
            generation,
            "an unchanged stack publishes nothing"
        );
    }

    #[test]
    fn a_go_across_a_shell_from_a_flat_page_places_it_with_no_placeholder_below() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.go("/sessions");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/sessions"]), strings(&["/sessions"])),
            "the flat page left, and the leaf replaced the inner navigator's root"
        );
        assert_eq!(h.inner.depth(), 1);
        assert!(!h.outer.can_pop());
        assert!(!h.outer.transition().active, "a go with no spec is a cut");
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn a_go_within_a_shell_placed_by_push_keeps_it_and_drops_the_page_below() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/sessions"]));

        h.router.go("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/sessions"]), strings(&["/terminal"]))
        );
        assert_eq!(
            h.shell_builds.get(),
            1,
            "a pushed shell page carries its key"
        );
        assert!(
            !h.outer.transition().active,
            "a retained top starts no flight"
        );
    }

    #[test]
    fn a_go_through_a_placed_shell_reaches_its_page_but_a_push_does_not() {
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let seen: Rc<RefCell<Option<String>>> = Rc::default();
        let shell_page = {
            let inner = inner.clone();
            let seen = seen.clone();
            move |params: &RouteParams| {
                seen.borrow_mut().clone_from(&params.get("n").cloned());
                any(navigator(&inner, || sized(11.0, 11.0)))
            }
        };
        let router = Router::with_controller(
            &outer,
            vec![shell_route(
                &inner,
                shell_page,
                vec![
                    Route::new("/sessions", |_| sized(20.0, 20.0)),
                    Route::new("/terminal", |_| sized(30.0, 30.0)),
                ],
            )],
        );
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = outer.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());
        router.go("/sessions?n=1");
        root.rebuild(&mut app, &mut ());
        assert_eq!(seen.borrow().as_deref(), Some("1"));
        let generation = outer.route_generation();

        router.go("/terminal?n=2");
        root.rebuild(&mut app, &mut ());
        assert_eq!(seen.borrow().as_deref(), Some("2"));
        assert_eq!(outer.route_generation(), generation);
        assert_eq!(paths(&outer), strings(&["/sessions"]));

        router.push("/sessions?n=3");
        root.rebuild(&mut app, &mut ());
        assert_eq!(seen.borrow().as_deref(), Some("2"));
    }

    #[test]
    fn a_go_to_a_shell_matched_as_the_leaf_leaves_the_inner_navigator_alone() {
        // Declared before `/`, a pathless shell matches the empty location itself.
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let builds = Rc::new(Cell::new(0u32));
        let shell_page = {
            let inner = inner.clone();
            let builds = builds.clone();
            move |_: &RouteParams| {
                let builds = builds.clone();
                any(crate::column()
                    .child(BuildCounter { builds })
                    .child(navigator(&inner, || sized(11.0, 11.0))))
            }
        };
        let router = Router::with_controller(
            &outer,
            vec![shell_route(
                &inner,
                shell_page,
                vec![Route::new("/sessions", |_| sized(20.0, 20.0))],
            )],
        );
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = outer.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());
        router.go("/sessions");
        root.rebuild(&mut app, &mut ());

        router.go("/");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&inner), strings(&["/sessions"]));
        assert_eq!(outer.depth(), 1);
        assert_eq!(builds.get(), 1);
    }

    #[test]
    fn a_go_to_a_shell_matched_as_the_leaf_beats_an_earlier_go_in_one_frame() {
        let mut h = ShellHarness::with_routes(|inner, shell_page| {
            vec![shell_route(
                inner,
                shell_page,
                vec![
                    Route::new("/sessions", |_| sized(20.0, 20.0)),
                    Route::new("/terminal", |_| sized(30.0, 30.0)),
                ],
            )]
        });
        h.router.go("/sessions");
        h.rebuild();

        h.router.go("/terminal");
        h.router.go("/");
        h.rebuild();
        assert_eq!(
            paths(&h.inner),
            strings(&["/sessions"]),
            "the last go names no inner page, so the inner stays as it was"
        );
        assert_eq!(h.outer.depth(), 1);
        assert_eq!(h.shell_builds.get(), 1);

        // The earlier go's note went with its op: a push reads the inner afresh.
        h.router.go("/terminal");
        h.router.go("/");
        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(paths(&h.inner), strings(&["/sessions", "/terminal"]));
        assert_eq!(h.outer.depth(), 1);
    }

    #[test]
    fn a_go_between_children_below_a_parent_page_publishes_nothing_on_the_outer_stack() {
        let mut h = ShellHarness::under_root();
        h.router.go("/settings");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/settings"]));
        let generation = h.outer.route_generation();

        h.router.go("/inbox?tab=a");
        h.rebuild();
        assert_eq!(
            h.outer.route_generation(),
            generation,
            "the parent's stamp did not move and the shell keeps its own"
        );
        assert_eq!(h.stacks().0, strings(&["/", "/settings"]));
        assert_eq!(h.stacks().1, strings(&["/inbox"]));
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn a_shell_popped_out_of_a_go_chain_is_placed_again_by_a_push() {
        let mut h = ShellHarness::under_root();
        h.router.go("/settings");
        h.rebuild();

        h.router.pop();
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/"]));
        assert_eq!(
            h.outer
                .route_stack()
                .current_route()
                .map(|route| route.path.as_str()),
            Some("/"),
            "the page left behind reads as itself, not as the child it was kept for"
        );
        assert!(!h.inner.is_mounted());

        h.router.push("/inbox");
        h.rebuild();
        assert_eq!(h.shell_builds.get(), 2, "the shell page was placed again");
        assert!(
            h.inner.is_mounted(),
            "so the push did not strand on the inner"
        );
        assert_eq!(h.stacks().1, strings(&["/inbox"]));
        assert_eq!(h.inner.depth(), 1);
        assert_eq!(
            h.stacks().0.last().map(String::as_str),
            Some("/inbox"),
            "the shell page is on top of the outer stack"
        );
    }

    #[test]
    fn a_push_and_a_replace_that_place_a_shell_stamp_the_parent_with_its_own_path() {
        let mut h = ShellHarness::under_root();
        h.router.go("/flat");
        h.rebuild();
        h.router.push("/inbox");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/flat", "/", "/inbox"]));
        assert_eq!(h.shell_builds.get(), 1);

        let mut h = ShellHarness::under_root();
        h.router.go("/flat");
        h.rebuild();
        h.router.replace("/inbox");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/inbox"]));

        // The stamps still read as placed: this push stays inside the shell.
        h.router.push("/settings");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/inbox"]));
        assert_eq!(h.stacks().1, strings(&["/inbox", "/settings"]));
        assert_eq!(h.shell_builds.get(), 1);
    }

    /// Leave a placed shell in the frame that also queued a push inside it, then
    /// come back in through `re_enter`.
    fn go_out_with_a_stale_push_then_back_in(re_enter: impl FnOnce(&Router<()>)) -> ShellHarness {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        h.router.push("/sessions");
        h.rebuild();

        h.router.push("/terminal");
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/"]));
        assert!(!h.inner.is_mounted());

        re_enter(&h.router);
        h.rebuild();
        h
    }

    #[test]
    fn a_go_out_of_a_shell_discards_a_stale_push_before_a_push_back_in() {
        let h = go_out_with_a_stale_push_then_back_in(|router| router.push("/sessions"));
        assert_eq!(h.stacks().0, strings(&["/", "/sessions"]));
        assert_eq!(
            h.stacks().1,
            strings(&["/sessions"]),
            "the stale push did not land on the next mount"
        );
        assert_eq!(h.inner.depth(), 1);
    }

    #[test]
    fn a_go_out_of_a_shell_discards_a_stale_push_before_a_go_back_in() {
        let h = go_out_with_a_stale_push_then_back_in(|router| router.go("/sessions"));
        assert_eq!(h.stacks().0, strings(&["/sessions"]));
        assert_eq!(h.stacks().1, strings(&["/sessions"]));
        assert_eq!(h.inner.depth(), 1);
    }

    #[test]
    fn go_then_go_in_one_frame_lands_on_the_last() {
        let mut h = PumpHarness::new(pump_routes());
        h.router.go("/home");
        h.router.go("/detail");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(20.0, 20.0))]);
        assert_eq!(paths(h.router.controller()), strings(&["/detail"]));
    }

    #[test]
    fn go_there_and_back_in_one_frame_keeps_the_pages_state() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let observed = Rc::new(Cell::new(0u32));
        let router = {
            let obs = observed.clone();
            Router::with_controller(
                &controller,
                vec![
                    Route::new("/a", move |_| counter(&obs)),
                    Route::new("/b", |_| sized(20.0, 20.0)),
                ],
            )
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = controller.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        router.go("/a");
        let ms = settle(&mut root, &mut app, 0);
        root.event(&mut (), &down(5.0, 5.0));
        let ms = settle(&mut root, &mut app, ms);
        assert_eq!(observed.get(), 1);

        router.go("/b");
        router.go("/a");
        settle(&mut root, &mut app, ms);
        assert_eq!(observed.get(), 1, "the last go alone decides what stays");
    }

    #[test]
    fn a_page_pushed_with_a_result_callback_before_a_go_in_one_frame_still_fires_it() {
        let mut h = PumpHarness::new(pump_routes());
        let results = Rc::new(RefCell::new(Vec::new()));
        h.router.push_with("/detail", recording_options(&results));
        h.router.go("/other");
        let _ = h.frame();
        let _ = h.frame();
        assert_eq!(results.borrow().len(), 1);
        assert_eq!(paths(h.router.controller()), strings(&["/other"]));
    }

    #[test]
    fn go_into_out_of_and_back_into_a_shell_in_one_frame_keeps_the_last_leaf() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.go("/sessions");
        h.router.go("/");
        h.router.go("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/terminal"]), strings(&["/terminal"])),
            "no shell page built in passing took the last leaf down with it"
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn go_into_a_shell_then_out_in_one_frame_leaves_nothing_on_its_queue() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.go("/sessions");
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/"]));
        assert_eq!(
            h.shell_builds.get(),
            0,
            "the first go was superseded unbuilt"
        );
        assert!(!h.inner.is_mounted());

        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/", "/terminal"]), strings(&["/terminal"]))
        );
    }

    #[test]
    fn go_then_go_within_a_shell_in_one_frame_places_it_once() {
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.router.go("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/terminal"]), strings(&["/terminal"]))
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn go_into_a_shell_then_push_in_one_frame_reads_the_go() {
        // The committed stack has no shell yet; the go says it will.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.go("/sessions");
        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (
                strings(&["/sessions"]),
                strings(&["/sessions", "/terminal"])
            )
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn go_out_of_a_shell_then_push_in_one_frame_reads_the_go() {
        // The committed stack still has the shell; the go says it will not.
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();

        h.router.go("/");
        h.router.push("/terminal");
        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/terminal"]));
        assert_eq!(
            h.stacks().1,
            strings(&["/terminal", "/sessions"]),
            "one fresh shell page, with both pushes on its inner navigator"
        );
        assert_eq!(h.shell_builds.get(), 2);
    }

    #[test]
    fn push_into_a_shell_then_go_in_one_frame_places_it_once() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.push("/sessions");
        h.router.go("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/terminal"]), strings(&["/terminal"])),
            "the go's own list placed the shell, and the push was superseded"
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn push_into_a_shell_then_go_out_in_one_frame_forgets_the_placement() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.push("/sessions");
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/"]));

        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/", "/terminal"]), strings(&["/terminal"])),
            "the shell was placed again, not assumed from the dropped placement"
        );
    }

    #[test]
    fn push_in_go_out_and_push_in_again_in_one_frame_places_the_shell_afresh() {
        // The go ends the first push's placement note, so the second push does not
        // read it and stack onto a shell the go just dropped.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();

        h.router.push("/sessions");
        h.router.go("/");
        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(
            h.stacks(),
            (strings(&["/", "/terminal"]), strings(&["/terminal"]))
        );
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn an_identical_stack_go_does_not_wedge_later_keep_decisions() {
        // No publish, so no new generation, to retire the go's record.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        let generation = h.outer.route_generation();
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.outer.route_generation(), generation);
        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/sessions"]));

        // And with the shell on the stack, a push still goes inside it.
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();
        let generation = h.outer.route_generation();
        h.router.go("/sessions");
        h.rebuild();
        assert_eq!(h.outer.route_generation(), generation);
        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/sessions"]));
        assert_eq!(h.stacks().1, strings(&["/sessions", "/terminal"]));

        // A go there and back in one frame also leaves the stack identical.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        let generation = h.outer.route_generation();
        h.router.go("/sessions");
        h.router.go("/");
        h.rebuild();
        assert_eq!(h.outer.route_generation(), generation);
        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/sessions"]));
    }

    #[test]
    fn a_placement_undone_in_its_own_frame_does_not_wedge_the_keep_rule() {
        // A push and a pop in one frame publish nothing, so no generation retires
        // the placement noted for the push.
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        let generation = h.outer.route_generation();
        h.router.push("/sessions");
        h.router.pop();
        h.rebuild();
        assert_eq!(h.outer.route_generation(), generation);

        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/", "/sessions"]));
    }

    #[test]
    fn a_placement_an_unreleased_swipe_holds_back_still_counts_as_placed() {
        // The swipe withholds the route-stack publish of a batch already drained,
        // so neither the queue nor the committed stack can answer here.
        let mut h = ShellHarness::new();
        h.app = {
            let c = h.outer.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)).pop_swipe(true))
        };
        h.router.go("/");
        h.router.push("/flat");
        let ms = settle(&mut h.root, &mut h.app, 0);
        h.root.event(&mut (), &down(5.0, 50.0));
        settle(&mut h.root, &mut h.app, ms + 200);
        h.root.event(
            &mut (),
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(30.0, 50.0),
                button: PointerButton::Primary,
            }),
        );
        assert!(h.outer.transition().interactive);

        h.router.push("/sessions");
        h.rebuild();
        assert!(h.outer.transition().interactive, "still held back");
        h.router.push("/terminal");
        h.rebuild();
        assert_eq!(h.shell_builds.get(), 1, "no second shell page was placed");
        assert_eq!(h.stacks().1, strings(&["/sessions", "/terminal"]));
    }

    #[test]
    fn a_go_to_an_unmatched_location_makes_the_stack_one_error_page() {
        let builds = Rc::new(Cell::new(0u32));
        let counted = builds.clone();
        let controller: NavigatorController<()> = NavigatorController::new();
        let router = Router::with_controller(&controller, pump_routes()).error_builder(move |_| {
            BuildCounter {
                builds: counted.clone(),
            }
        });
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = controller.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());
        router.go("/home");
        root.rebuild(&mut app, &mut ());
        router.push("/detail");
        root.rebuild(&mut app, &mut ());
        assert_eq!(controller.depth(), 2);

        router.go("/nope");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&controller), strings(&["-"]));
        assert_eq!(builds.get(), 1);

        router.go("/nope");
        root.rebuild(&mut app, &mut ());
        assert_eq!(builds.get(), 1, "the same bad location keeps its page");

        router.go("/other-nope");
        root.rebuild(&mut app, &mut ());
        assert_eq!(builds.get(), 2);
        assert_eq!(controller.depth(), 1);

        router.go("/home");
        root.rebuild(&mut app, &mut ());
        router.push("/detail");
        root.rebuild(&mut app, &mut ());
        router.go_named("no-such-route", &RouteParams::new());
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&controller), strings(&["-"]), "an unknown name too");
        assert_eq!(builds.get(), 3);
    }

    #[test]
    fn a_go_to_an_unmatched_location_leaves_the_shell_and_discards_its_queue() {
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();

        h.router.push("/terminal");
        h.router.go("/nope");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["-"]));
        assert!(!h.inner.is_mounted());

        h.router.push("/sessions");
        h.rebuild();
        assert_eq!(h.stacks().1, strings(&["/sessions"]));
    }

    #[test]
    fn an_unmatched_location_handled_as_a_link_pushes_and_back_returns() {
        let mut h = PumpHarness::new(pump_routes());
        let controller = h.router.controller().clone();
        h.router.go("/home");
        let _ = h.frame();
        h.router.push("/detail");
        let _ = h.frame();

        h.router.handle_location("/nope");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(99.0, 99.0))]);
        assert_eq!(paths(&controller), strings(&["/home", "/detail", "-"]));

        h.router.pop();
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(20.0, 20.0))]);
        assert_eq!(paths(&controller), strings(&["/home", "/detail"]));

        // A go from app code still replaces the stack with the one error page.
        h.router.go("/nope");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(99.0, 99.0))]);
        assert_eq!(paths(&controller), strings(&["-"]));
        assert_eq!(controller.depth(), 1);

        // And a matched link is still a go.
        h.router.handle_location("/other");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(30.0, 30.0))]);
        assert_eq!(paths(&controller), strings(&["/other"]));
    }

    #[test]
    fn an_unmatched_location_handled_as_a_link_leaves_a_placed_shell_alone() {
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();

        h.router.push("/terminal");
        h.router.handle_location("/nope");
        h.rebuild();
        assert_eq!(h.stacks().0, strings(&["/sessions", "-"]));
        assert_eq!(
            h.stacks().1,
            strings(&["/sessions", "/terminal"]),
            "the push queued in the same frame was not discarded"
        );
        assert!(h.inner.is_mounted());
        assert_eq!(h.shell_builds.get(), 1);
    }

    #[test]
    fn a_go_through_a_shell_in_a_shell_keeps_both_and_sets_the_innermost() {
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner_a: NavigatorController<()> = NavigatorController::new();
        let inner_b: NavigatorController<()> = NavigatorController::new();
        let builds_a = Rc::new(Cell::new(0u32));
        let builds_b = Rc::new(Cell::new(0u32));
        let page_a = {
            let (inner, builds) = (inner_a.clone(), builds_a.clone());
            move |_: &RouteParams| {
                let builds = builds.clone();
                any(crate::column()
                    .child(BuildCounter { builds })
                    .child(navigator(&inner, || sized(11.0, 11.0))))
            }
        };
        let page_b = {
            let (inner, builds) = (inner_b.clone(), builds_b.clone());
            move |_: &RouteParams| {
                let builds = builds.clone();
                any(crate::column()
                    .child(BuildCounter { builds })
                    .child(navigator(&inner, || sized(12.0, 12.0))))
            }
        };
        let routes = vec![
            Route::new("/", |_| sized(10.0, 10.0)),
            shell_route(
                &inner_a,
                page_a,
                vec![shell_route(
                    &inner_b,
                    page_b,
                    vec![
                        Route::new("/one", |_| sized(20.0, 20.0)),
                        Route::new("/two", |_| sized(30.0, 30.0)),
                    ],
                )],
            ),
        ];
        let router = Router::with_controller(&outer, routes);
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app: Build = {
            let c = outer.clone();
            Box::new(move |_: &mut ()| navigator(&c, || sized(10.0, 10.0)))
        };
        root.rebuild(&mut app, &mut ());

        router.go("/one");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&outer), strings(&["/one"]));
        assert_eq!(paths(&inner_a), strings(&["/one"]));
        assert_eq!(paths(&inner_b), strings(&["/one"]));
        let generations = (outer.route_generation(), inner_a.route_generation());

        router.go("/two");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&inner_b), strings(&["/two"]));
        assert_eq!(
            (outer.route_generation(), inner_a.route_generation()),
            generations,
            "neither enclosing stack changed"
        );
        assert_eq!((builds_a.get(), builds_b.get()), (1, 1));

        // A push within reads the same keep decision at both boundaries.
        router.push("/one");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&inner_b), strings(&["/two", "/one"]));
        assert_eq!((outer.depth(), inner_a.depth()), (1, 1));

        // Out of the outer shell: both inner stacks' queues are discarded.
        router.push("/two");
        router.go("/");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&outer), strings(&["/"]));
        assert!(!inner_a.is_mounted() && !inner_b.is_mounted());

        router.push("/two");
        root.rebuild(&mut app, &mut ());
        assert_eq!(paths(&outer), strings(&["/", "/two"]));
        assert_eq!(paths(&inner_b), strings(&["/two"]));
    }

    #[test]
    fn a_route_navigator_go_through_pump_behaves_the_same() {
        let mut h = PumpHarness::new(pump_routes());
        let nav = h.router.route_navigator();
        nav.go("/home");
        let _ = h.frame();
        nav.push("/detail");
        let _ = h.frame();
        assert_eq!(h.router.controller().depth(), 2);

        nav.go("/other");
        assert_eq!(h.frame(), vec![(Point::ZERO, Size::new(30.0, 30.0))]);
        assert_eq!(paths(h.router.controller()), strings(&["/other"]));
    }

    #[test]
    fn a_go_from_inside_a_placed_shell_to_an_animated_flat_route_flies_with_the_shell_leaving() {
        let mut h = ShellHarness::new();
        h.router.go("/sessions");
        h.rebuild();

        h.router.go("/flat");
        h.rebuild();
        let flight = h.outer.transition();
        assert!(flight.active && !flight.is_pop, "a push flight, not a cut");
        assert_eq!(h.stacks().0, strings(&["/flat"]));
        assert_eq!(h.outer.depth(), 1, "committed on the go's own frame");
        assert!(!h.outer.can_pop());
        assert!(
            h.inner.is_mounted(),
            "the leaving shell page is still alive"
        );

        settle(&mut h.root, &mut h.app, 0);
        assert!(!h.outer.transition().active);
        assert!(!h.inner.is_mounted(), "the shell page left at settle");
        assert_eq!(h.outer.depth(), 1);
    }

    #[test]
    fn a_go_from_a_flat_route_to_an_animated_flat_route_flies() {
        let mut h = ShellHarness::new();
        h.router.go("/");
        h.rebuild();
        h.router.push("/flat");
        settle(&mut h.root, &mut h.app, 0);
        assert_eq!(h.outer.depth(), 2);

        h.router.go("/");
        settle(&mut h.root, &mut h.app, 1_000);
        h.router.go("/flat");
        h.rebuild();
        let flight = h.outer.transition();
        assert!(flight.active && !flight.is_pop);
        assert_eq!(h.stacks().0, strings(&["/flat"]));
        assert_eq!(h.outer.depth(), 1, "committed on the go's own frame");
    }

    // ================= Go: the keyboard =================

    /// A field-shaped leaf: claims focus on `Down` and republishes its surface on
    /// every paint while it holds focus, as a real text input does.
    struct FieldLeaf;
    struct FieldLeafWidget;
    fn field_surface() -> frust_core::ImeState {
        frust_core::ImeState {
            active: true,
            editing: frust_core::EditingState {
                text: "query".to_string(),
                selection_base: 5,
                selection_extent: 5,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
            content_type: Default::default(),
            suppress_soft_keyboard: false,
        }
    }
    impl View<()> for FieldLeaf {
        type Element = FieldLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FieldLeafWidget {
            FieldLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FieldLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for FieldLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            if ctx.has_focus() {
                ctx.publish_ime_state(field_surface());
            }
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.request_focus();
                ctx.publish_ime_state(field_surface());
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn a_go_to_the_current_page_from_a_focused_field_keeps_the_keyboard_session() {
        let mut h = PumpHarness::new(vec![
            Route::new("/field", |_| FieldLeaf),
            Route::new("/other", |_| sized(30.0, 30.0)),
        ]);
        h.router.go("/field");
        let _ = h.frame();
        h.root.event(&mut h.state, &down(5.0, 5.0));
        let _ = h.frame();
        assert!(h.root.is_focus_active());

        h.router.go("/field?from=link");
        let _ = h.frame();
        assert!(h.root.is_focus_active(), "the session survived the go");
        assert!(h.root.ime_state().is_some());

        h.router.go("/other");
        let _ = h.frame();
        assert!(!h.root.is_focus_active(), "leaving the page ends it");
    }

    // ================= Per-route and per-push transitions =================

    use super::super::transition::{PageTransition, REDUCE_MOTION_DURATION, Timing, resolve_spec};

    fn slide() -> TransitionSpec {
        TransitionSpec::duration(PageTransition::IosPush)
    }

    /// Seed a navigator with one page, build a router over `routes`, run `op`
    /// against it, rebuild once, and report whether a page transition started.
    fn transition_started(
        routes: Vec<Route<()>>,
        navigator_default: TransitionSpec,
        theme: Option<Theme>,
        op: impl FnOnce(&Router<()>),
    ) -> bool {
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(&controller, routes);
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        if let Some(theme) = theme {
            root.set_theme(Box::new(theme));
        }
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized(10.0, 10.0)).transition(navigator_default)
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        assert!(
            !controller.transition().active,
            "seeding starts no transition"
        );
        op(&router);
        root.rebuild(&mut app, &mut state);
        controller.transition().active
    }

    fn plain_routes() -> Vec<Route<()>> {
        vec![
            Route::new("/a", |_| sized(10.0, 10.0)),
            Route::new("/b", |_| sized(20.0, 20.0)),
        ]
    }

    #[test]
    fn route_transition_applies_to_push_and_replace() {
        let routes = || {
            vec![
                Route::new("/a", |_| sized(10.0, 10.0)).transition(slide()),
                Route::new("/b", |_| sized(20.0, 20.0)),
            ]
        };
        assert!(transition_started(
            routes(),
            TransitionSpec::NONE,
            None,
            |r| r.push("/a")
        ));
        assert!(transition_started(
            routes(),
            TransitionSpec::NONE,
            None,
            |r| r.replace("/a")
        ));
        assert!(!transition_started(
            routes(),
            TransitionSpec::NONE,
            None,
            |r| r.push("/b")
        ));
    }

    #[test]
    fn route_without_transition_uses_the_navigator_default() {
        assert!(!transition_started(
            plain_routes(),
            TransitionSpec::NONE,
            None,
            |r| r.push("/a")
        ));
        assert!(transition_started(plain_routes(), slide(), None, |r| r.push("/a")));
    }

    #[test]
    fn route_transition_beats_the_navigator_default() {
        let routes = vec![Route::new("/a", |_| sized(10.0, 10.0)).transition(TransitionSpec::NONE)];
        assert!(!transition_started(routes, slide(), None, |r| r.push("/a")));
    }

    #[test]
    fn push_with_transition_overrides_the_route_transition() {
        let routes = vec![Route::new("/a", |_| sized(10.0, 10.0)).transition(TransitionSpec::NONE)];
        assert!(transition_started(
            routes,
            TransitionSpec::NONE,
            None,
            |r| r.push_with("/a", PushOptions::opaque().transition(slide()))
        ));
    }

    #[test]
    fn push_with_no_transition_falls_back_to_the_route_transition() {
        let routes = vec![Route::new("/a", |_| sized(10.0, 10.0)).transition(slide())];
        assert!(transition_started(
            routes,
            TransitionSpec::NONE,
            None,
            |r| r.push_with("/a", PushOptions::opaque())
        ));
        assert!(!transition_started(
            plain_routes(),
            TransitionSpec::NONE,
            None,
            |r| r.push_with("/a", PushOptions::opaque())
        ));
    }

    #[test]
    fn go_animates_only_the_new_top_with_its_own_routes_transition() {
        // The new top flies on its own route's spec; a spec on a page below it
        // starts nothing.
        let chain = |parent: TransitionSpec, child: TransitionSpec| {
            vec![
                Route::new("/p", |_| sized(10.0, 10.0))
                    .transition(parent)
                    .child(Route::new("c", |_| sized(20.0, 20.0)).transition(child)),
            ]
        };
        assert!(transition_started(
            chain(TransitionSpec::NONE, slide()),
            TransitionSpec::NONE,
            None,
            |r| r.go("/p/c")
        ));
        assert!(!transition_started(
            chain(slide(), TransitionSpec::NONE),
            TransitionSpec::NONE,
            None,
            |r| r.go("/p/c")
        ));
        assert!(!transition_started(
            chain(TransitionSpec::NONE, TransitionSpec::NONE),
            TransitionSpec::NONE,
            None,
            |r| r.go("/p/c")
        ));
    }

    #[test]
    fn push_with_options_apply_to_the_leaf_only() {
        let routes = vec![
            Route::new("/p", |_| sized(10.0, 10.0))
                .child(Route::new("c", |_| sized(20.0, 20.0)).transition(TransitionSpec::NONE)),
        ];
        // The leaf's own route spec is NONE, but the caller's slide wins on it.
        assert!(transition_started(
            routes,
            TransitionSpec::NONE,
            None,
            |r| r.push_with("/p/c", PushOptions::opaque().transition(slide()))
        ));
    }

    #[test]
    fn push_with_still_stamps_the_location() {
        let controller: NavigatorController<()> = NavigatorController::new();
        let router: Router<()> = Router::with_controller(&controller, plain_routes());
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut ()| navigator(&ctrl, || sized(10.0, 10.0))
        };
        let mut state = ();
        root.rebuild(&mut app, &mut state);
        router.push_with(
            "/b",
            PushOptions::opaque()
                .route(Location::parse("/ignored"))
                .transition(slide()),
        );
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.route_stack().entries().last(),
            Some(&Some(Location::parse("/b")))
        );
        assert_eq!(controller.route_stack().change(), NavChange::Push);
    }

    #[test]
    fn route_transition_reaches_the_op_under_reduce_motion_and_collapses() {
        let mut reduced = Theme::neutral();
        reduced.motion.reduce_motion = true;
        let motion = reduced.motion;
        let routes = vec![Route::new("/a", |_| sized(10.0, 10.0)).transition(slide())];
        assert!(transition_started(
            routes,
            TransitionSpec::NONE,
            Some(reduced),
            |r| r.push("/a")
        ));
        let resolved = resolve_spec(slide(), Some(&motion));
        assert_eq!(resolved.preset, PageTransition::ReducedCrossfade);
        assert_eq!(
            resolved.timing,
            Timing::Duration(REDUCE_MOTION_DURATION, frust_core::Curve::Linear)
        );
    }
}
