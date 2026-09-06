//! [`NavigatorView`] — the declarative navigator descriptor — and its two
//! constructors, [`navigator`] and [`overlay_host`].
//!
//! The `View` impl itself (build/rebuild/teardown, where the op queue is
//! drained) stays with the widget core in [`navigator`](super::navigator).

use std::rc::Rc;

use frust_core::AnyView;

use super::controller::NavigatorController;
use super::options::{PageBuilder, PageVisibility, RouteChangeCallback, VisibilityCallback};
use super::path::Location;
use super::route_state::RouteStack;
use super::transition::{PageTransition, TransitionSpec};

// Named only by the doc comments moved here with this module's items, so their
// intra-doc links keep resolving to the same targets they did in `navigator`.
#[allow(unused_imports)]
use super::navigator::NavigatorWidget;
#[allow(unused_imports)]
use super::options::{BackPolicy, PushOptions};
#[allow(unused_imports)]
use frust_core::{View, Widget};

/// A declarative navigator. See the [module docs](self).
pub struct NavigatorView<State: 'static> {
    pub(super) controller: NavigatorController<State>,
    pub(super) initial: PageBuilder<State>,
    /// The transition applied to a push/replace that supplies no per-op override.
    /// Defaults to [`TransitionSpec::NONE`] (instant switches).
    pub(super) default_transition: TransitionSpec,
    /// Explicit override for the interactive edge-swipe back gesture — the
    /// highest-ranked slot in [`resolve_pop_swipe`](Self::resolve_pop_swipe)'s
    /// resolution (page → navigator explicit → platform → preset). `None`
    /// defers to [`platform_pop_swipe`](Self::platform_pop_swipe), and past
    /// that to the default transition preset — on for
    /// [`PageTransition::IosPush`], off otherwise.
    pub(super) pop_swipe: Option<bool>,
    /// **Facade-only** platform-derived default, ranked below an explicit
    /// [`pop_swipe`](Self::pop_swipe) override and above the preset-derived
    /// fallback. `frust-widgets` carries no `cfg(target_os)` of its own — this
    /// is the plain setter the facade calls with `cfg!(target_os = "ios")`
    /// (`crates/frust/src/lib.rs`'s `navigator`/`overlay_host` wrappers).
    /// `None` until set.
    pub(super) platform_pop_swipe: Option<bool>,
    /// The ROOT page's [`PageVisibility`] observer (the root has no
    /// [`PushOptions`] to carry one). Installed on the root page entry at
    /// `build`, like the root's [`BackPolicy`] — not live-refreshed.
    pub(super) root_visibility: Option<VisibilityCallback>,
    /// Whether a [`Covered`](PageVisibility::Covered) page stops re-running its
    /// builder. Default `false` — the shipped behaviour.
    pub(super) cull_covered_builds: bool,
    /// The ROOT page's route identity — the root has no [`PushOptions`]
    /// to carry [`PushOptions::route`], so this mirrors
    /// [`root_visibility`](Self::root_visibility)'s shape. Installed on the
    /// root page entry at `build`, like the root's [`BackPolicy`].
    pub(super) root_route: Option<Location>,
    /// Navigator-wide route-change observer — see
    /// [`on_route_change`](Self::on_route_change). Unlike `root_visibility`
    /// this is refreshed on every rebuild (live-configurable, like
    /// `default_transition`), since it observes the whole navigator rather
    /// than one page.
    pub(super) route_change: Option<RouteChangeCallback>,
}

impl<State: 'static> NavigatorView<State> {
    /// Set the default page transition applied to every push/replace that does
    /// not carry its own [`push_with`](NavigatorController::push_with)/
    /// [`replace_with`](NavigatorController::replace_with) override.
    pub fn transition(mut self, spec: TransitionSpec) -> Self {
        self.default_transition = spec;
        self
    }

    /// Explicitly enable or disable the interactive edge-swipe back gesture,
    /// outranking both [`platform_pop_swipe`](Self::platform_pop_swipe) and the
    /// preset-derived default (on for [`PageTransition::IosPush`], off
    /// otherwise). The gesture pops the top page with a left-edge drag: drag
    /// progress reverses the popped page's transition, and release completes
    /// or cancels the pop by progress/velocity. A page may still narrow this
    /// further with [`PushOptions::pop_swipe`] (the highest-ranked slot) or
    /// refuse it outright via a non-[`Pop`](BackPolicy::Pop) `BackPolicy` (see
    /// [`NavigatorWidget::swipe_armable`]).
    pub fn pop_swipe(mut self, enabled: bool) -> Self {
        self.pop_swipe = Some(enabled);
        self
    }

    /// Set the platform-derived edge-swipe default — ranked below an explicit
    /// [`pop_swipe`](Self::pop_swipe) override and above the preset-derived
    /// fallback. `frust-widgets` carries no `cfg(target_os)` of its own; this
    /// is the plain setter the facade calls (`crates/frust/src/lib.rs`) with
    /// `cfg!(target_os = "ios")`, so an app using `frust::navigator` gets an
    /// iOS-on / Android-and-desktop-off default with zero app-side wiring.
    pub fn platform_pop_swipe(mut self, enabled: bool) -> Self {
        self.platform_pop_swipe = Some(enabled);
        self
    }

    /// Observe the **root** page's [`PageVisibility`] — the same seam
    /// [`PushOptions::on_visibility`] gives a pushed page, for the one page that
    /// has no `PushOptions`. Fired once with
    /// [`Current`](PageVisibility::Current) on the navigator's first build, then
    /// on every change (e.g. [`Covered`](PageVisibility::Covered) when an opaque
    /// page is pushed over it).
    ///
    /// Read [`PushOptions::on_visibility`]'s doc for the full contract: no
    /// `&mut State`, no duplicate values, and — importantly — **no `on_cleanup`
    /// on cover**; the root page stays mounted with its widget state intact.
    ///
    /// The callback is captured at the navigator's first `build` (like the root
    /// page's builder itself) and is not refreshed on later rebuilds.
    pub fn on_root_visibility(mut self, f: impl Fn(PageVisibility) + 'static) -> Self {
        self.root_visibility = Some(Rc::new(f));
        self
    }

    /// Skip re-running the builder (and the child reconcile) for pages that are
    /// [`Covered`](PageVisibility::Covered).
    ///
    /// **Default `false`** — the shipped behaviour, where every retained page
    /// reconciles every frame whether covered or not. Making covered builds stop
    /// is a real behaviour change (an app may rely on a covered page's builder
    /// running against live state), so it is strictly opt-in.
    ///
    /// Two ordering rules hold when enabled, and neither needs a wake mechanism:
    ///
    /// - **A revealed page rebuilds in the same pass that revealed it.** Ops are
    ///   applied — and a settled transition finalized — *before* the per-page
    ///   reconcile loop in [`NavigatorView::rebuild`], so by the time the loop
    ///   asks [`visibility_of`](NavigatorWidget::visibility_of) the revealed page
    ///   is no longer `Covered`. There is no cross-frame gap to bridge.
    /// - **The frame on which a page *becomes* `Covered` still rebuilds it.** The
    ///   cull decision reads the page's visibility as of the *previous* reconcile,
    ///   so a page gets exactly one final reconcile after its
    ///   `on_visibility(Covered)` fires — a page staging teardown UI on cover can
    ///   still render it.
    pub fn cull_covered_builds(mut self, enabled: bool) -> Self {
        self.cull_covered_builds = enabled;
        self
    }

    /// Stamp the ROOT page's route identity — see
    /// [`PushOptions::route`], the pushed-page equivalent.
    pub fn root_route(mut self, location: Location) -> Self {
        self.root_route = Some(location);
        self
    }

    /// Observe this navigator's route-state, navigator-wide: fired from
    /// [`NavigatorWidget::publish_state`] only when the published
    /// [`RouteStack`] actually changes (an unchanged stack across N rebuilds
    /// fires it zero times) — never with `&mut State` (it fires from a
    /// rebuild), exactly like [`PushOptions::on_visibility`]'s contract. The
    /// facade's reactive route-observer bridges this into signals (see
    /// `docs/WIDGETS_ARCHITECTURE.md`'s reactive-free note): capture a plain
    /// `Rc<Cell<_>>` here, don't reach for app state.
    pub fn on_route_change(mut self, f: impl Fn(&RouteStack) + 'static) -> Self {
        self.route_change = Some(Rc::new(f));
        self
    }

    /// Resolve the navigator-wide edge-swipe default: the explicit override if
    /// set, else [`platform_pop_swipe`](Self::platform_pop_swipe) if set, else
    /// on for the iOS-push preset (the transition the swipe is designed around)
    /// and off for every other default. A page's own
    /// [`PushOptions::pop_swipe`] outranks this at the arm site — see
    /// [`NavigatorWidget::swipe_armable`], which is what `event_at` actually
    /// consults.
    pub(super) fn resolve_pop_swipe(&self) -> bool {
        self.pop_swipe
            .or(self.platform_pop_swipe)
            .unwrap_or(self.default_transition.preset == PageTransition::IosPush)
    }
}

/// Build a [`NavigatorView`] driven by `controller`, whose initial (root) page is
/// produced by `initial`. The app-facing entry point (see [module docs](self)).
pub fn navigator<State: 'static>(
    controller: &NavigatorController<State>,
    initial: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    NavigatorView {
        controller: controller.clone(),
        initial: Rc::new(initial),
        default_transition: TransitionSpec::NONE,
        pop_swipe: None,
        platform_pop_swipe: None,
        root_visibility: None,
        cull_covered_builds: false,
        root_route: None,
        route_change: None,
    }
}

/// The **root overlay host**: a navigator whose root page is the whole app —
/// chrome, tab shell, inner navigator and all — and whose pushed pages are the
/// app's modals. Because the host sits *above* every piece of chrome, an overlay
/// pushed here dims and blocks chrome that an overlay on an inner navigator
/// cannot reach.
///
/// It is a [`navigator`] with two defaults changed and nothing else:
///
/// * **[`pop_swipe(false)`](NavigatorView::pop_swipe)** — an edge swipe must
///   never dismiss an overlay.
/// * **[`TransitionSpec::NONE`]** — each overlay widget stages its *own*
///   enter/exit (the contract the dialog/sheet catalogs already rely on), so the
///   host must not animate the page swap underneath them.
///
/// Everything else is the ordinary navigator, deliberately: dismiss-signal
/// routing, [`PushOptions`]/[`BackPolicy`], per-overlay
/// [`on_result`](PushOptions::on_result), the keyboard drop on a page switch,
/// and the capture-cancel + focus-clear are inherited rather than re-invented.
///
/// ```no_run
/// # use frust_widgets::{NavigatorController, overlay_host, text};
/// # use frust_core::any;
/// # let controller: NavigatorController<()> = NavigatorController::new();
/// # let app_root = || any(text("the whole app: chrome, tabs, inner navigator"));
/// // Wrap the app's existing root view; nothing inside it changes.
/// let root = overlay_host(&controller, move || app_root());
/// # let _ = root;
/// ```
///
/// # The host owns no scrim
///
/// The per-page-paints-its-own-scrim convention is preserved verbatim: the host
/// is purely structural and paints nothing of its own. An overlay page already
/// fills `ctx.origin()..ctx.size()` with its scrim, and at the root that rect
/// *is* the window — so the catalogs' dialogs and sheets need no change to dim
/// the whole app.
///
/// # Chrome inertness is not new code
///
/// [`NavigatorWidget::event`](Widget::event) routes to
/// [`input_routed_pages`](NavigatorWidget::input_routed_pages) — the top page
/// only — so with an overlay up the entire app root, chrome included, receives
/// nothing. The accessibility tree says the same thing through the same
/// function under rule R23 (see [`semantics`](Widget::semantics)), so there is
/// no second reachability path to keep in sync.
///
/// # Back arbitration
///
/// With a root host *and* an inner navigator there are two back registrants. The
/// facade's back glue (`frust::back_glue`) routes a press to the first
/// registrant claiming [`back_interest`](NavigatorController::back_interest),
/// ranked host-first and then innermost-first among plain navigators — so a
/// press with a root overlay open reaches the host, and a press with none open
/// falls through to the inner navigator (a host at depth 1 with the default
/// [`BackPolicy::Pop`] claims nothing). An open overlay additionally silences
/// the inner navigator outright: the host's root page — the whole app — is no
/// longer input-routed, so under the R23 back-reach gate (see the module docs)
/// nothing inside it claims the press either.
pub fn overlay_host<State: 'static>(
    controller: &NavigatorController<State>,
    app: impl Fn() -> AnyView<State> + 'static,
) -> NavigatorView<State> {
    navigator(controller, app)
        // Both are stated explicitly rather than left to the `navigator`
        // defaults: they are the host's *contract*, not a coincidence of what
        // `navigator` happens to default to.
        .pop_swipe(false)
        .transition(TransitionSpec::NONE)
}
