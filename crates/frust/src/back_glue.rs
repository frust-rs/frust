//! Back-press ⇄ navigator glue (predictive-back + facade
//! auto-wiring): the facade is the only crate
//! that sees both `frust-widgets`' [`NavigatorController`] and `frust-reactive`'s
//! back-press source together — mirroring [`router_glue`](crate::router_glue)
//! exactly (`frust-widgets` stays reactive-free, `frust-reactive` stays
//! navigator-free).
//!
//! # The Android back contract
//!
//! A shell delivers a hardware/gesture back press → the framework attempts an
//! async back → the framework maintains a "handles back" boolean the shell reads
//! so a *root-level* back falls through to the platform (activity finish). This
//! module is the framework half. A back press is consumed exactly once and
//! routed through [`NavigatorController::request_back`], which applies
//! the top page's `BackPolicy` — `Pop` pops, `DismissAnimated` fires the
//! overlay's dismiss signal, `Veto` swallows it — rather than a bare
//! [`pop`](NavigatorController::pop). The "handles back" answer is computed from
//! [`back_interest`](NavigatorController::back_interest) (not
//! [`can_pop`](NavigatorController::can_pop)), so a dismissable/veto overlay at
//! the *root* still claims the press ahead-of-time (predictive-back parity)
//! instead of letting it exit the app.
//!
//! # Two entry points, one consumption source
//!
//! Back handling reaches a controller two ways, and both funnel through the same
//! process-wide consumption marker so a press is consumed *exactly once*:
//!
//! - **Automatic**: the facade's [`navigator`](crate::navigator)
//!   wrapper calls [`auto_wire`] on every rebuild, so any app using
//!   `frust::navigator` gets back handling with ZERO back-specific app code.
//! - **Explicit**: [`BackHandler`]/[`attach_back_handler`] — the original
//!   surface an app constructs in `Component::init` and drives with
//!   [`track`](BackHandler::track) from every `Component::build`. Still fully
//!   supported (Huddle uses it); it now shares the same consumption marker and
//!   `request_back`/`back_interest` routing as the automatic path.
//!
//! A single process-wide (UI-thread-affine) [`SHARED`] marker is the "single
//! consumption source": when Huddle constructs a [`BackHandler`] *and* calls
//! `frust::navigator` on the same controller, whichever runs first in a given
//! rebuild consumes the press; the other observes the same already-consumed
//! count and no-ops, so one press = one `request_back` (no double-pop).
//!
//! # R44-back: arbitration across more than one navigator
//!
//! An app with a root [`overlay_host`](crate::overlay_host) has **two**
//! navigators wired at once — the host, and the inner navigator inside it — and
//! both want the back press. `frust-reactive`'s live-provider slot stays
//! single-registrant (see `frust_reactive::back`); this module owns that one
//! registration and multiplexes behind it:
//!
//! > **R44-back — a root overlay host outranks a plain navigator, and equal
//! > ranks order by first wire; a back press goes to the first registrant
//! > reporting [`back_interest()`](NavigatorController::back_interest)` ==
//! > true`.**
//!
//! [`SharedBack::registrants`] is that ordered list, kept sorted by
//! ([`Role`], first-wire sequence). **Rank comes from the call site, not from
//! wire timing**: [`auto_wire_overlay_host`] (the facade's
//! [`overlay_host`](crate::overlay_host)) registers [`Role::Host`], every other
//! entry point registers [`Role::Navigator`]. That is what makes the order
//! independent of *when* and *how many times* a controller wires — the two
//! things an app genuinely varies:
//!
//! - An explicit [`BackHandler`] constructed in `Component::init` wires its
//!   controller **before any view exists**, so first-wire order alone would put
//!   an inner navigator ahead of a host that cannot possibly have wired yet.
//! - The documented Huddle pattern wires one controller **twice in a single
//!   build pass** (`state.back.track()` and `frust::navigator(&controller, …)`),
//!   so "one wire per pass" is not a premise anything here may rest on.
//!
//! `handles_back`/the live provider answer `any(interest)`; a consumed press
//! routes to the *first* registrant with interest. **The single-navigator case
//! is unchanged bit for bit**: one registrant makes `any` and "the first with
//! interest" both degenerate to that controller, whatever its rank. So does a
//! host with no overlays — at depth 1 with the default `BackPolicy::Pop`,
//! `compute_back_interest` is `false`, and the host transparently defers to the
//! inner navigator.
//!
//! ## Releasing a navigator that went away
//!
//! A navigator that goes away (a screen with its own nested navigator, popped)
//! must not keep claiming presses, and must not keep its controller clone alive
//! forever. Two rules drop it, in priority order:
//!
//! 1. **Mounted is live.** [`NavigatorController::is_mounted`] is exact — the
//!    navigator's `View::build`/`teardown` write it — so a *mounted* registrant
//!    is in the retained tree by definition and is never pruned, no matter how
//!    the wire sequence looks. This is the load-bearing half: a root overlay
//!    host is mounted from its own `build` (which runs before its page builder,
//!    hence before the inner navigator wires at all), so nothing an inner
//!    navigator does to the wire sequence can evict it.
//! 2. **The one-cycle rule, for registrants that were never mounted.** A wire
//!    from an already-registered controller ends a window; an unmounted
//!    registrant that did not wire within it is dropped. This covers the only
//!    case rule 1 cannot see — a controller wired through a `BackHandler` whose
//!    navigator view never made it into the tree.
//!
//! Rule 2 is deliberately *not* trusted for mounted navigators: it infers a
//! build cycle from a repeat wire, and the Huddle pattern above breaks that
//! inference (the second wire of one pass looks exactly like the first wire of
//! the next). Before rule 1 existed, that inference pruned the root overlay
//! host mid-pass and re-created finding #44.
//!
//! # Timing: the live provider closes the stale-window
//!
//! The wiring runs during a rebuild's *build* pass, **before** the navigator's
//! `apply_ops` (a later reconciliation step) publishes the new stack depth — so
//! the polled [`set_handles_back`] flag it writes lags a stack change by one
//! frame, and if the frame gate skips the settle frame that stale value can
//! persist. To close that window, the wiring also registers a live provider
//! ([`set_can_pop_provider`]) reading `controller.back_interest()`, which
//! `frust_reactive::handles_back` queries in preference to the flag: it reads
//! the navigator's CURRENT interest at press time (after `apply_ops` published
//! it), so a back press is always decided against the real state, with no
//! one-frame lag. The provider is UI-thread-affine (it reads an `Rc`-backed
//! controller through [`SHARED`]). See `frust_reactive::back`'s module docs for
//! the source-side contract.

use std::cell::RefCell;
use std::rc::Rc;

use frust_reactive::{CanPopRegistration, back_presses, set_can_pop_provider, set_handles_back};
use frust_widgets::{NavigatorController, NavigatorId};
use reactive_graph::traits::{Get, GetUntracked};

thread_local! {
    /// The process-wide (UI-thread-affine) back-press wiring shared by BOTH the
    /// automatic [`navigator`](crate::navigator) path and the explicit
    /// [`BackHandler`]. A single shared marker is the "single consumption
    /// source" that makes an auto-wired `navigator()` and a manual
    /// `BackHandler` on the same controller consume a press EXACTLY once — no
    /// double-pop — and a single ordered registrant list is what arbitrates a
    /// press across a root overlay host and the navigator inside it (R44-back;
    /// see the module docs).
    ///
    /// UI-thread-affine because every [`Registrant`] holds an `Rc`-backed
    /// `NavigatorController` (`!Send`), exactly like frust-reactive's live
    /// provider slot it feeds.
    static SHARED: RefCell<SharedBack> = const { RefCell::new(SharedBack::new()) };
}

/// What a registrant *is*, which is what R44-back ranks it by — taken from the
/// facade entry point that wired it, never inferred from wire timing.
///
/// `Ord` is the arbitration order: [`Host`](Self::Host) sorts first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Role {
    /// A root [`overlay_host`](crate::overlay_host). Its overlays are drawn over
    /// every piece of chrome, including any inner navigator, so an open one owns
    /// the back press outright — the #44 contract.
    Host,
    /// Any other wired navigator: [`navigator`](crate::navigator)'s auto-wiring
    /// or an explicit [`BackHandler`].
    Navigator,
}

/// One wired navigator in [`SharedBack::registrants`] — the unit R44-back
/// arbitrates over. The closures hold a controller clone, so a registrant keeps
/// its navigator's op queue alive until it is pruned.
struct Registrant {
    /// Which controller this entry stands for, so a re-wire refreshes it in
    /// place instead of appending a duplicate.
    id: NavigatorId,
    /// This entry's arbitration rank (see [`Role`]). Sticky-upgrades to
    /// [`Role::Host`]: a controller wired *once* as an overlay host is one, even
    /// on a re-wire that names it through a different entry point.
    role: Role,
    /// The wire sequence number of this entry's FIRST wire — the tiebreak within
    /// a role, so two navigators of equal rank keep build order.
    birth: u64,
    /// The wire sequence number of this entry's most recent refresh; the
    /// one-cycle prune compares against it (see [`refresh_interest`]).
    seq: u64,
    /// `controller.is_mounted()` — is this navigator in the retained tree right
    /// now? A `true` here vetoes the one-cycle prune (see the module docs'
    /// release rules).
    mounted: Box<dyn Fn() -> bool>,
    /// `controller.back_interest()` — does this navigator claim the next press?
    interest: Box<dyn Fn() -> bool>,
    /// `controller.request_back()` — deliver the press to this navigator,
    /// honoring its top page's `BackPolicy`. An `Rc` rather than a `Box` so
    /// [`route_back`] can clone it out and call it with **no** [`SHARED`] borrow
    /// outstanding.
    request_back: Rc<dyn Fn()>,
}

impl Registrant {
    /// The sort key R44-back orders the arbitration list by: rank first, then
    /// first-wire order within a rank.
    fn key(&self) -> (Role, u64) {
        (self.role, self.birth)
    }
}

/// The shared back state behind [`SHARED`] (see its docs).
struct SharedBack {
    /// The last back-press count consumed by *any* entry point; `None` until
    /// the first observation. A press up to this count has already been routed
    /// through `request_back`, so a second entry point (or a re-run rebuild)
    /// observing the same count does not re-fire (the `RouterDeepLinks`
    /// consumed-marker pattern, shared across both back entry points).
    consumed: Option<u64>,
    /// Every wired navigator, sorted by ([`Registrant::role`],
    /// [`Registrant::birth`]) — hosts first, then build order — which is the
    /// R44-back arbitration list. Kept sorted on every wire, so a registrant's
    /// position never depends on how often its controller wires per pass; an
    /// unmounted registrant that misses a build cycle is pruned (see
    /// [`refresh_interest`]). The live provider (below) reads through this list,
    /// so a press is decided against the CURRENT navigators' interest.
    registrants: Vec<Registrant>,
    /// Monotonic wire counter stamped into [`Registrant::birth`]/
    /// [`Registrant::seq`]; ordering and the one-cycle prune are both expressed
    /// against it.
    seq: u64,
    /// The live can-pop provider registration, made once on the first wire and
    /// kept for the process lifetime. Its closure reads
    /// [`registrants`](Self::registrants) live, so `handles_back()` answers
    /// against the current navigators with no one-frame lag (see the module
    /// docs' timing note). `None` until the first wire registers it.
    provider: Option<CanPopRegistration>,
}

impl SharedBack {
    const fn new() -> Self {
        Self {
            consumed: None,
            registrants: Vec::new(),
            seq: 0,
            provider: None,
        }
    }
}

/// Whether **any** wired navigator claims the next back press — the value both
/// the live provider and the polled `handles_back` flag publish.
///
/// Only ever called with no outstanding [`SHARED`] borrow: an `interest` probe
/// reads an `Rc<Cell<bool>>` on its controller and never re-enters this module.
fn any_interest() -> bool {
    SHARED.with(|shared| shared.borrow().registrants.iter().any(|r| (r.interest)()))
}

/// Register (or refresh) `controller` in the R44-back arbitration list at
/// `role`'s rank and republish the polled `handles_back` fallback flag,
/// registering the live provider once. Called on every wire from both entry
/// points ([`auto_wire`]/[`auto_wire_overlay_host`] and
/// [`BackHandler::new`]/[`track`](BackHandler::track)).
///
/// Ordering and pruning are the whole mechanism — see the module docs' R44-back
/// section. In short: the entry is (re-)inserted at its ([`Role`], first-wire)
/// sort position, so neither *when* nor *how often* a controller wires can move
/// it; and a repeat wire drops every **unmounted** registrant that did not wire
/// since this controller's previous wire, a mounted one being in the tree by
/// definition.
///
/// # Panics
///
/// The first call registers the live provider via [`set_can_pop_provider`],
/// which panics off the UI thread — always the case here (a rebuild / a
/// `Component::init` runs on the UI thread).
fn refresh_interest<State: 'static>(controller: &NavigatorController<State>, role: Role) {
    let id = controller.id();
    let mounted_probe = controller.clone();
    let interest_probe = controller.clone();
    let request_probe = controller.clone();
    SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        shared.seq += 1;
        let seq = shared.seq;
        // Carry the existing entry's identity forward (rank sticky-upgrades to
        // `Host`, first-wire order is preserved), and prune against its previous
        // wire while it is still in the list.
        let (role, birth) = match shared.registrants.iter().position(|r| r.id == id) {
            Some(index) => {
                let previous = shared.registrants[index].seq;
                let carried = (
                    // `Host` sorts first, so `min` IS the sticky upgrade: a
                    // controller ever wired as an overlay host stays one.
                    shared.registrants[index].role.min(role),
                    shared.registrants[index].birth,
                );
                // A repeat wire closes a window. Any registrant that neither
                // wired within it nor is currently mounted is no longer in the
                // tree — drop it rather than let it keep claiming presses. The
                // mounted probe is a `Cell` read on the registrant's controller
                // and never re-enters this module, so calling it under the
                // `SHARED` borrow is safe (same contract as `any_interest`).
                shared
                    .registrants
                    .retain(|r| r.id == id || r.seq >= previous || (r.mounted)());
                carried
            }
            // First wire: this seq is the entry's permanent tiebreak.
            None => (role, seq),
        };
        let entry = Registrant {
            id,
            role,
            birth,
            seq,
            mounted: Box::new(move || mounted_probe.is_mounted()),
            interest: Box::new(move || interest_probe.back_interest()),
            request_back: Rc::new(move || request_probe.request_back()),
        };
        // Re-insert at the sort position, never "wherever it already was": that
        // is what keeps arbitration order a function of what a registrant IS
        // (its role and first wire) rather than of the wire sequence.
        shared.registrants.retain(|r| r.id != id);
        let at = shared
            .registrants
            .iter()
            .position(|r| r.key() > entry.key())
            .unwrap_or(shared.registrants.len());
        shared.registrants.insert(at, entry);
        if shared.provider.is_none() {
            // Register the stable live provider ONCE. It reads whatever
            // registrants are currently wired from `SHARED`, so a new
            // controller joining the list needs no re-registration.
            shared.provider = Some(set_can_pop_provider(Box::new(any_interest)));
        }
    });
    // Refresh the polled fallback flag (the live provider is authoritative; this
    // only keeps the no-provider fallback roughly in sync — see the module docs).
    set_handles_back(any_interest());
}

/// Deliver a consumed back press under **R44-back**: to the first registrant in
/// arbitration order (hosts first, then build order) that claims it via
/// `back_interest()`.
///
/// `fallback` is the controller whose wire consumed the press, used only when
/// *no* registrant claims one — which preserves the pre-arbitration behaviour
/// exactly (a press was always delivered to the wiring controller). Delivery
/// there is a no-op by construction: `back_interest()` is false only at depth 1
/// with a `BackPolicy::Pop` top page, and `request_back` on that stack pops
/// nothing.
fn route_back<State: 'static>(fallback: &NavigatorController<State>) {
    // The claimant's `request_back` is CLONED OUT of the list (that is what the
    // `Rc<dyn Fn()>` is for) and called with NO `SHARED` borrow outstanding.
    // `request_back` only enqueues an op or bumps a dismiss signal today, but it
    // is the one call here that reaches app-reachable machinery, and a re-entry
    // into this module would panic on the already-borrowed `RefCell` — so the
    // borrow ends before the call rather than that staying load-bearing. (The
    // `interest` probe above is a plain `Cell` read, as `any_interest` notes.)
    let claimant = SHARED.with(|shared| {
        let shared = shared.borrow();
        shared
            .registrants
            .iter()
            .find(|r| (r.interest)())
            .map(|r| Rc::clone(&r.request_back))
    });
    match claimant {
        Some(request_back) => request_back(),
        None => fallback.request_back(),
    }
}

/// Consume any new back press against the shared marker, routing it through
/// [`NavigatorController::request_back`] (applies the top page's `BackPolicy`)
/// on the navigator [`route_back`] arbitrates to. The tracked read of the
/// back-press counter subscribes THIS rebuild, so a later `push_back_press`
/// wakes it (State & Reactivity: the track-per-rebuild contract).
///
/// The shared marker is the single consumption source (see the module docs): if
/// both entry points run in the same rebuild on the same controller, the first
/// fires `request_back` and the second no-ops on the already-consumed count.
///
/// `controller` is the *wiring* controller, not necessarily the recipient —
/// under R44-back the press goes to the first registrant claiming it, and
/// `controller` is only the fallback when none does.
fn consume_back<State: 'static>(controller: &NavigatorController<State>) {
    // Tracked read — subscribes the rebuild to the back-press counter.
    let count = back_presses().count.get();
    let fire = SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        match shared.consumed {
            // A fresh process/marker: seed to the current count WITHOUT firing,
            // so a press delivered before any handler existed does not trigger a
            // spurious back on the first wire.
            None => {
                shared.consumed = Some(count);
                false
            }
            // Already caught up — a re-run rebuild or the second entry point.
            Some(prev) if prev == count => false,
            // A new press (a burst collapses to one `request_back`: catch up to
            // the current depth, don't replay each event — the back semantics).
            Some(_) => {
                shared.consumed = Some(count);
                true
            }
        }
    });
    if fire {
        route_back(controller);
    }
}

/// Seed the shared consumption marker to the current back-press count if it is
/// not seeded yet, so a press delivered before a [`BackHandler`] existed does
/// not trigger a spurious `request_back` on the first [`track`](BackHandler::track).
/// Idempotent (only-if-unseeded), so it composes with a `navigator()` that
/// already seeded the marker on an earlier rebuild.
fn seed_consumed() {
    let count = back_presses().count.get_untracked();
    SHARED.with(|shared| {
        let mut shared = shared.borrow_mut();
        if shared.consumed.is_none() {
            shared.consumed = Some(count);
        }
    });
}

/// Auto-wire back handling for `controller`: the entry point the
/// facade's [`navigator`](crate::navigator) wrapper calls on every rebuild.
/// Consumes a new back press (routing it through `request_back`) and refreshes
/// the `handles_back` interest — so an app using `frust::navigator` gets the
/// full Android back contract (overlay dismiss → pop → app exit) with ZERO
/// back-specific app code, and with no double-pop against a manual
/// [`BackHandler`] on the same controller (see the module docs).
///
/// # Panics
///
/// Runs during a rebuild's build pass; the first call registers the live
/// provider, which panics off the UI thread (always the UI thread here).
pub(crate) fn auto_wire<State: 'static>(controller: &NavigatorController<State>) {
    consume_back(controller);
    refresh_interest(controller, Role::Navigator);
}

/// [`auto_wire`] for a root [`overlay_host`](crate::overlay_host): identical,
/// except the registrant takes [`Role::Host`] — the rank that puts an open
/// root overlay ahead of the inner navigator whatever the wire sequence looks
/// like (R44-back; see the module docs).
///
/// # Panics
///
/// As [`auto_wire`].
pub(crate) fn auto_wire_overlay_host<State: 'static>(controller: &NavigatorController<State>) {
    consume_back(controller);
    refresh_interest(controller, Role::Host);
}

/// A [`NavigatorController`] wired to the process-wide back-press source (see
/// the module docs) — the **explicit** back-wiring surface predating the
/// automatic [`navigator`](crate::navigator) auto-wiring.
///
/// App code using `frust::navigator` no longer needs this: back handling is
/// automatic. It stays fully supported for apps that want an explicit handle
/// (Huddle constructs one), and now shares the same single consumption source
/// and `request_back`/`back_interest` routing as the automatic path — so
/// constructing a `BackHandler` *and* calling `frust::navigator` on the same
/// controller still consumes each press exactly once.
///
/// Construct once with [`attach_back_handler`] (or [`BackHandler::new`]) —
/// typically from `Component::init`, storing the result in `Component::State` —
/// then call [`track`](Self::track) from every `Component::build`.
pub struct BackHandler<State: 'static> {
    controller: NavigatorController<State>,
}

impl<State: 'static> BackHandler<State> {
    /// Wire `controller` to the back-press source. Seeds the shared consumption
    /// marker to the current back-press count (so a press delivered before this
    /// handler existed does not trigger a spurious back on the first
    /// [`track`](Self::track)), publishes the initial `handles_back` fallback
    /// flag, and registers the live `back_interest` provider (see the module
    /// docs' timing note).
    ///
    /// Call this once per controller (e.g. from `Component::init`) — from the UI
    /// thread, since the provider slot is UI-thread-affine (it reads the
    /// `Rc`-backed controller). A `Component::init` always runs on the UI thread.
    ///
    /// **Registration position does not depend on when you construct this**
    /// (see the module docs' R44-back section): a handler built in
    /// `Component::init` wires before any view exists, and a root
    /// [`overlay_host`](crate::overlay_host) still outranks the navigator it
    /// wraps — the host's rank comes from its own entry point, not from wire
    /// order. Where you call [`track`](Self::track) from is likewise free.
    pub fn new(controller: NavigatorController<State>) -> Self {
        seed_consumed();
        refresh_interest(&controller, Role::Navigator);
        Self { controller }
    }

    /// Consume a new back press (routing it through
    /// [`request_back`](NavigatorController::request_back)) and refresh the
    /// framework's `handles_back` interest from the current stack. Call from
    /// every `Component::build`.
    ///
    /// Shares the single consumption source with the automatic
    /// [`navigator`](crate::navigator) path (see the module docs): a rebuild
    /// re-run that observes the same already-consumed count neither re-fires nor
    /// double-counts, and a `frust::navigator` on the same controller in the
    /// same rebuild cannot double-pop.
    pub fn track(&self) {
        consume_back(&self.controller);
        refresh_interest(&self.controller, Role::Navigator);
    }

    /// The wired controller — hand it to
    /// [`navigator`](frust_widgets::navigator), or drive it directly.
    pub fn controller(&self) -> &NavigatorController<State> {
        &self.controller
    }
}

/// Convenience constructor equivalent to [`BackHandler::new`] — see its docs and
/// [`RouterDeepLinks`](crate::RouterDeepLinks)/[`router_with_deep_links`](crate::router_with_deep_links)
/// for the mirrored API shape.
///
/// Note that back handling is **automatic** for any app using
/// [`frust::navigator`](crate::navigator); this explicit surface is
/// only needed when an app wants a handle it drives directly (e.g. Huddle).
///
/// ```no_run
/// use frust::{
///     AnyView, BackHandler, Component, NavigatorController, any, attach_back_handler,
///     navigator, text,
/// };
///
/// #[derive(Default)]
/// struct App;
///
/// struct AppState {
///     back: BackHandler<AppState>,
/// }
///
/// impl Component for App {
///     type State = AppState;
///
///     fn init(&self) -> AppState {
///         let nav: NavigatorController<AppState> = NavigatorController::new();
///         AppState {
///             back: attach_back_handler(nav),
///         }
///     }
///
///     fn build(&self, state: &mut AppState) -> AnyView<AppState> {
///         // Called every rebuild: consumes a new back press (via `request_back`)
///         // and keeps the framework's handles-back interest in sync. The
///         // `frust::navigator` call auto-wires the SAME controller — one press
///         // still pops exactly once (shared consumption source).
///         state.back.track();
///         let controller = state.back.controller();
///         any(navigator(controller, || any(text("home"))))
///     }
/// }
///
/// frust::app!(App);
/// # fn main() {}
/// ```
pub fn attach_back_handler<State: 'static>(
    controller: NavigatorController<State>,
) -> BackHandler<State> {
    BackHandler::new(controller)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{AnyView, RenderRoot, any};
    use frust_reactive::{
        ReactiveRuntime, clear_can_pop_provider, handles_back, push_back_press, set_handles_back,
    };
    use frust_widgets::{
        BackPolicy, NavigatorController, NavigatorView, PushOptions, navigator as raw_navigator,
        overlay_host as raw_overlay_host,
    };
    use std::sync::{Arc, Mutex};

    /// Serializes every test in this module: they all touch `frust-reactive`'s
    /// process-wide back-press counter / `handles_back` flag / live-provider slot
    /// AND this module's `SHARED` marker, so concurrent runs would race the
    /// shared counter (one test's `push_back_press` advancing it under another's
    /// feet). `reset()` (below) additionally clears the per-thread state a reused
    /// harness thread would otherwise carry between tests.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Reset the per-thread wiring so each test starts clean: force-clear
    /// frust-reactive's live provider, drop the shared marker/interest/provider,
    /// and reset the polled flag. Call at the top of every test (after taking
    /// [`TEST_LOCK`], under an initialized runtime).
    fn reset() {
        clear_can_pop_provider();
        set_handles_back(false);
        SHARED.with(|shared| *shared.borrow_mut() = SharedBack::new());
    }

    fn page() -> AnyView<()> {
        any(frust_widgets::text("x"))
    }

    /// The `app_logic` closure type [`RenderRoot::rebuild`] drives, boxed so
    /// [`Harness`] can store it (mirroring `router_glue`'s `AppLogic`).
    type AppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

    /// A `RenderRoot`/app closure driving a controller's `navigator`. The
    /// `wire` closure runs each rebuild BEFORE reconciliation to exercise a back
    /// entry point (the automatic `auto_wire`, an explicit `BackHandler::track`,
    /// or both) exactly as a `Component::build` would.
    struct Harness {
        controller: NavigatorController<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: AppLogic,
        wire: Box<dyn FnMut()>,
    }

    impl Harness {
        /// A fresh-controller harness whose per-rebuild wiring runs `wire`
        /// (given a clone of that controller) — the seam each test uses to pick
        /// its back entry point.
        fn new(wire: impl FnMut(&NavigatorController<()>) + 'static) -> Self {
            Self::with_controller(NavigatorController::new(), wire)
        }

        /// As [`new`](Self::new) but drives an EXISTING `controller` — so a test
        /// can share one controller between a `BackHandler` and the navigator
        /// (the manual + auto criterion).
        fn with_controller(
            controller: NavigatorController<()>,
            mut wire: impl FnMut(&NavigatorController<()>) + 'static,
        ) -> Self {
            let app_controller = controller.clone();
            let app: AppLogic = Box::new(move |_: &mut ()| raw_navigator(&app_controller, page));
            let wire_controller = controller.clone();
            let wire: Box<dyn FnMut()> = Box::new(move || wire(&wire_controller));
            let mut harness = Harness {
                controller,
                root: RenderRoot::new(),
                app,
                wire,
            };
            harness.rebuild();
            harness
        }

        fn rebuild(&mut self) {
            // Wire under the build pass, exactly as a Component::build would,
            // then reconcile (which applies the navigator's queued ops).
            (self.wire)();
            self.root.rebuild(&mut self.app, &mut ());
        }

        fn depth(&self) -> usize {
            self.controller.depth()
        }
    }

    /// Criterion 1: an app using the AUTOMATIC `frust::navigator` auto-wiring
    /// (no `BackHandler` in app code) pops one page on a back press. Also
    /// covers criterion 2 (depth 1, no overlay → `handles_back()` false: the
    /// press falls through to the platform).
    #[test]
    fn auto_wire_pops_without_a_backhandler() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = Harness::new(auto_wire);

        // Criterion 2: at the root (depth 1, no overlay) the framework does NOT
        // handle back — a root-level press bubbles to the platform.
        assert_eq!(h.depth(), 1, "starts at the root page");
        assert!(!handles_back(), "depth 1 + no overlay: handles_back false");

        // Push to depth 2 (a normal opaque push → BackPolicy::Pop).
        h.controller.push(page);
        h.rebuild();
        assert_eq!(h.depth(), 2, "pushed to depth 2");
        assert!(
            handles_back(),
            "poppable stack: handles_back true, same rebuild"
        );

        // Criterion 1: one back press pops one page — with NO BackHandler.
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "auto-wired back press pops one page");
        assert!(
            !handles_back(),
            "back at the root: handles_back false again"
        );
    }

    /// Criterion 3: a `Veto` overlay claims the back press (`handles_back()`
    /// true — predictive-back parity via `back_interest`), the press is
    /// consumed, and the stack is UNCHANGED — the routing-through-`request_back`
    /// (not a bare `pop`) contract. This is the meaningful facade-reachable
    /// form of "depth 1 + Veto overlay": the overlay sits over the root, and
    /// even though a raw `pop()` *would* remove it (the stack is poppable),
    /// `request_back` honors the Veto policy and pops nothing. (A Veto policy on
    /// the depth-1 *root* itself is not expressible through the public push API
    /// — the root is always `BackPolicy::Pop` — and is covered by a
    /// widget-level `compute_back_interest` test.)
    #[test]
    fn veto_overlay_claims_back_but_does_not_pop() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = Harness::new(auto_wire);

        // Push a transparent Veto overlay over the root.
        h.controller
            .push_with_options(page, PushOptions::transparent().back(BackPolicy::Veto));
        h.rebuild();

        // The overlay claims back ahead-of-time (facade reads `back_interest`).
        assert!(handles_back(), "a Veto overlay claims the back press");

        let before = h.depth();
        // A back press is consumed but Veto swallows it — the stack is unchanged.
        // A bare `pop()` here would have removed the overlay (regression guard
        // for the request_back-not-pop switch).
        push_back_press();
        h.rebuild();
        assert_eq!(
            h.depth(),
            before,
            "Veto consumes the press via request_back: stack unchanged (not popped)"
        );
        assert!(handles_back(), "still claiming back (overlay still on top)");
    }

    /// The EXPLICIT `BackHandler` path still works on its own (regression for
    /// the original surface, now routing through `request_back`): a press
    /// pops one page and `handles_back()` tracks the depth with no settle frame
    /// (the live provider closes the stale-window).
    #[test]
    fn explicit_backhandler_pops_and_tracks_handles_back() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let controller: NavigatorController<()> = NavigatorController::new();
        let back = BackHandler::new(controller.clone());
        // Only the explicit handler drives back here — on the SAME controller the
        // harness's navigator uses (a raw navigator view, no auto-wiring).
        let mut h = Harness::with_controller(controller, move |_c| back.track());

        assert_eq!(h.depth(), 1, "starts at the root");
        assert!(
            !handles_back(),
            "root-level back bubbles: handles_back false"
        );

        h.controller.push(page);
        h.rebuild();
        assert_eq!(h.depth(), 2, "push published to depth immediately");
        assert!(
            handles_back(),
            "poppable stack reports handles_back true on the SAME rebuild, no settle frame"
        );

        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "one back press pops one page");
        assert!(
            !handles_back(),
            "root again: handles_back false immediately"
        );

        // A back press at the root is a safe no-op (consumed, pops nothing).
        push_back_press();
        h.rebuild();
        assert_eq!(h.depth(), 1, "pop-at-root is a no-op");

        // After a force-clear the polled fallback flag answers (depth-1 false).
        clear_can_pop_provider();
        assert!(
            !handles_back(),
            "after clear, handles_back reads the polled fallback"
        );
    }

    /// Criterion 4 (regression): a manual `BackHandler` AND the automatic
    /// `navigator()` auto-wiring on the SAME controller consume one press
    /// exactly once — no double-pop. Both run every rebuild (as Huddle does:
    /// `state.back.track()` then `frust::navigator(&controller, ...)`).
    #[test]
    fn manual_backhandler_plus_auto_wire_pops_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        // Construct the manual handler first (as an app would, in init).
        let controller: NavigatorController<()> = NavigatorController::new();
        let back = BackHandler::new(controller.clone());

        // The harness's navigator AND both back entry points drive the SAME
        // controller. Each rebuild runs BOTH entry points in Huddle's order: the
        // manual `track()` then the auto-wiring (`c` is the harness controller).
        let mut h = Harness::with_controller(controller, move |c| {
            back.track();
            auto_wire(c);
        });

        // Push to depth 3 so a single-vs-double pop is unambiguous.
        h.controller.push(page);
        h.rebuild();
        h.controller.push(page);
        h.rebuild();
        assert_eq!(h.depth(), 3, "pushed to depth 3");

        // ONE back press: with a shared consumption source this pops exactly one
        // page (depth 2). A double-consumption bug would pop two (depth 1).
        push_back_press();
        h.rebuild();
        assert_eq!(
            h.depth(),
            2,
            "manual + auto share one consumption source: one press pops once"
        );

        // A rebuild with no new press does not re-pop (dedupe).
        h.rebuild();
        assert_eq!(h.depth(), 2, "no new press -> no extra pop");
    }

    // -----------------------------------------------------------------------
    // R44-back — arbitration across a root overlay host and an inner navigator.
    // -----------------------------------------------------------------------

    /// A root `overlay_host` wrapping an inner `navigator`, each auto-wired
    /// where its view is constructed: the host from the app's build pass, the
    /// inner navigator from the host's root-page builder — which runs inside
    /// the host's own reconcile, i.e. strictly later. That *is* the build order
    /// R44-back arbitrates on, so the harness reproduces the real ordering
    /// rather than hand-declaring it.
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
                let host = host.clone();
                let inner = inner.clone();
                Box::new(move |_: &mut ()| {
                    // Outermost: exactly what `frust::overlay_host` does.
                    auto_wire_overlay_host(&host);
                    let inner = inner.clone();
                    raw_overlay_host(&host, move || {
                        // Innermost: exactly what `frust::navigator` does.
                        auto_wire(&inner);
                        any(raw_navigator(&inner, page))
                    })
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

    /// The R44-back arbitration list, in order — the whitebox view the ordering
    /// and pruning rules are pinned against.
    fn registrant_ids() -> Vec<frust_widgets::NavigatorId> {
        SHARED.with(|shared| shared.borrow().registrants.iter().map(|r| r.id).collect())
    }

    /// **R44-back**: with a root overlay open, the press goes to the HOST even
    /// though the inner navigator is poppable and wired *later*. Before
    /// arbitration the last wire won outright, so this press popped the inner
    /// navigator — a page vanishing under an open modal.
    #[test]
    fn a_root_overlay_takes_the_back_press_from_the_inner_navigator() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = HostHarness::new();
        // Registration order is build order: the host first, then the navigator
        // its root page builds.
        assert_eq!(
            registrant_ids(),
            vec![h.host.id(), h.inner.id()],
            "outermost first"
        );

        // Both stacks are poppable: the inner navigator has a page pushed, and
        // an overlay sits on the host.
        h.inner.push(page);
        h.rebuild();
        h.host
            .push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
        h.rebuild();
        assert_eq!((h.host.depth(), h.inner.depth()), (2, 2));
        assert!(handles_back(), "some registrant claims the press");

        push_back_press();
        h.rebuild();
        assert_eq!(h.host.depth(), 1, "the press went to the root overlay host");
        assert_eq!(
            h.inner.depth(),
            2,
            "and NOT to the inner navigator (the #44 back bug)"
        );
    }

    /// **The degenerate case, bit for bit.** A host with no overlays sits at
    /// depth 1 with the default `BackPolicy::Pop`, so `compute_back_interest`
    /// is false and it claims nothing — the press falls straight through to the
    /// inner navigator, exactly as before the host existed. This is the
    /// single-navigator (huddle) regression guard.
    #[test]
    fn a_host_with_no_overlay_defers_the_back_press_to_the_inner_navigator() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = HostHarness::new();
        assert!(
            !handles_back(),
            "two navigators, both at the root: back bubbles to the platform"
        );

        h.inner.push(page);
        h.rebuild();
        assert_eq!(h.host.depth(), 1, "the host stays empty");
        assert!(
            handles_back(),
            "the inner navigator's interest answers for the whole app"
        );

        push_back_press();
        h.rebuild();
        assert_eq!(h.inner.depth(), 1, "the press popped the inner navigator");
        assert_eq!(h.host.depth(), 1, "the empty host was untouched");
        assert!(!handles_back(), "back at the root of both: bubbles again");

        // A press with nothing to do stays a safe no-op (nobody claims it, so it
        // routes to the wiring controller, whose stack cannot pop).
        push_back_press();
        h.rebuild();
        assert_eq!((h.host.depth(), h.inner.depth()), (1, 1));
    }

    /// The predictive-back half of R44-back: an overlay on the host claims the
    /// press even when the inner navigator has nothing to pop. `handles_back`
    /// answers `any(interest)`, not "the last navigator wired" — under the
    /// latter the shell would be told nobody handles back and would *exit the
    /// app* with a root modal open.
    #[test]
    fn a_root_overlay_claims_back_even_with_the_inner_navigator_at_its_root() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = HostHarness::new();
        h.host
            .push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
        h.rebuild();
        assert_eq!((h.host.depth(), h.inner.depth()), (2, 1));
        assert!(
            handles_back(),
            "the root overlay claims the press though the inner navigator cannot pop"
        );

        push_back_press();
        h.rebuild();
        assert_eq!(h.host.depth(), 1, "the press dismissed the root overlay");
        assert!(!handles_back(), "nothing left to claim it");
    }

    /// The one-cycle prune: a navigator that stops wiring (its screen torn down)
    /// is dropped from the arbitration list rather than claiming presses
    /// forever from a stack nothing renders. These controllers are never mounted
    /// (no widget builds them), so the mounted veto never applies and this is
    /// the pure one-cycle path.
    #[test]
    fn a_navigator_that_stops_wiring_is_pruned_after_one_full_cycle() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let nested: NavigatorController<()> = NavigatorController::new();

        // Frame 1: three navigators wire, in build order.
        refresh_interest(&outer, Role::Navigator);
        refresh_interest(&inner, Role::Navigator);
        refresh_interest(&nested, Role::Navigator);
        assert_eq!(
            registrant_ids(),
            vec![outer.id(), inner.id(), nested.id()],
            "appended in build order"
        );

        // Frame 2: the nested navigator's screen is gone — it never wires again.
        refresh_interest(&outer, Role::Navigator);
        refresh_interest(&inner, Role::Navigator);
        assert_eq!(
            registrant_ids(),
            vec![outer.id(), inner.id(), nested.id()],
            "not yet: a full cycle has not elapsed without it"
        );

        // Frame 3: the outer wire now spans a whole cycle in which `nested`
        // never appeared.
        refresh_interest(&outer, Role::Navigator);
        assert_eq!(
            registrant_ids(),
            vec![outer.id(), inner.id()],
            "the departed navigator is pruned, and its controller clone released"
        );

        // Re-wiring an existing registrant never re-orders it.
        refresh_interest(&inner, Role::Navigator);
        refresh_interest(&outer, Role::Navigator);
        refresh_interest(&inner, Role::Navigator);
        assert_eq!(registrant_ids(), vec![outer.id(), inner.id()]);
    }

    // -----------------------------------------------------------------------
    // F1: the two ways the pre-fix "a repeat wire proves a build cycle" premise
    // broke — a controller wiring TWICE per pass, and a `BackHandler` wiring
    // from `Component::init` before any view exists.
    // -----------------------------------------------------------------------

    /// The REAL Huddle shape under a root overlay host: the inner navigator is
    /// wired **twice in one build pass** — an explicit `BackHandler` built in
    /// `Component::init` and `track()`ed from the host's page builder, plus the
    /// automatic `navigator()` wiring right beside it
    /// (`examples/huddle/src/lib.rs` does exactly this pair).
    ///
    /// Before the fix, the second wire's one-cycle prune read the FIRST wire of
    /// the same pass as a cycle boundary and dropped the host, which then
    /// re-appended *after* the inner navigator — permanently inverting
    /// arbitration and re-creating finding #44 (a back press with a root modal
    /// open popping the page underneath it).
    #[test]
    fn an_overlay_host_survives_an_inner_navigator_wiring_twice_per_pass() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let host: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        // `Component::init`: the explicit handler exists before any view does.
        let back = Rc::new(BackHandler::new(inner.clone()));

        let mut app: AppLogic = {
            let host = host.clone();
            let inner = inner.clone();
            Box::new(move |_: &mut ()| {
                // The app's build pass: `frust::overlay_host`.
                auto_wire_overlay_host(&host);
                let inner = inner.clone();
                let back = Rc::clone(&back);
                raw_overlay_host(&host, move || {
                    // The host's page builder — Huddle's own build body, moved
                    // inside the host: `state.back.track()` then
                    // `frust::navigator(&controller, …)`, one pass, one
                    // controller, TWO wires.
                    back.track();
                    auto_wire(&inner);
                    any(raw_navigator(&inner, page))
                })
            })
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());

        assert_eq!(
            registrant_ids(),
            vec![host.id(), inner.id()],
            "the host outranks the inner navigator despite wiring once to its twice"
        );

        // An overlay over the whole app, and a poppable inner stack under it.
        inner.push(page);
        host.push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
        // Two more passes: the ordering must survive repeated double-wiring.
        for _ in 0..2 {
            root.rebuild(&mut app, &mut ());
        }
        assert_eq!((host.depth(), inner.depth()), (2, 2));
        assert_eq!(
            registrant_ids(),
            vec![host.id(), inner.id()],
            "still outermost-first after repeated passes"
        );
        assert!(
            handles_back(),
            "the shell is told the app handles back (a root modal is open)"
        );

        push_back_press();
        root.rebuild(&mut app, &mut ());
        assert_eq!(host.depth(), 1, "the press dismissed the root overlay");
        assert_eq!(
            inner.depth(),
            2,
            "and NOT the page under it (finding #44's back bug)"
        );
    }

    /// The second ordering break: `BackHandler::new` wires from
    /// `Component::init`, which runs before ANY view is constructed — so a
    /// first-wire-order rule alone gave the inner navigator position 0 from
    /// frame 1, and no amount of "track from inside the host's page builder"
    /// advice could fix a registration that already happened. Rank comes from
    /// the entry point instead, so the host still sorts first.
    #[test]
    fn an_init_registered_backhandler_does_not_outrank_a_later_overlay_host() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let host: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();

        // `Component::init` — the inner navigator's handler registers FIRST.
        let back = BackHandler::new(inner.clone());
        assert_eq!(registrant_ids(), vec![inner.id()], "init wired it alone");

        // The first `Component::build`: the host wires only now.
        auto_wire_overlay_host(&host);
        assert_eq!(
            registrant_ids(),
            vec![host.id(), inner.id()],
            "the host claims the outermost slot even though it wired second"
        );

        // And a repeat pass keeps it there.
        auto_wire_overlay_host(&host);
        back.track();
        assert_eq!(registrant_ids(), vec![host.id(), inner.id()]);
    }

    /// The mounted veto's other half: it only ever *delays* release. A
    /// navigator whose widget is torn down (its page rebuilt without it) reports
    /// unmounted, so the one-cycle rule drops it and releases the controller
    /// clone the registrant holds.
    #[test]
    fn a_torn_down_navigator_stops_vetoing_the_prune_and_is_released() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let outer: NavigatorController<()> = NavigatorController::new();
        let nested: NavigatorController<()> = NavigatorController::new();
        // The nested navigator lives inside the outer navigator's root page,
        // until this flag turns it off.
        let show_nested = Rc::new(std::cell::Cell::new(true));
        // The arbitration list as seen MID-PASS, at the moment the outer
        // navigator's page rebuilds — i.e. inside the window a spurious prune
        // opens, when the shell may read `handles_back` or a press may route.
        let mid_pass: Rc<RefCell<Vec<NavigatorId>>> = Rc::new(RefCell::new(Vec::new()));

        let mut app: AppLogic = {
            let outer = outer.clone();
            let nested = nested.clone();
            let show_nested = Rc::clone(&show_nested);
            let mid_pass = Rc::clone(&mid_pass);
            Box::new(move |_: &mut ()| {
                // The outer navigator wires TWICE per pass (Huddle's
                // `track()` + `navigator()` pair), so the one-cycle rule alone
                // would prune the nested navigator on every second wire — the
                // mounted veto is the only reason it survives below.
                auto_wire(&outer);
                auto_wire(&outer);
                let nested = nested.clone();
                let show_nested = Rc::clone(&show_nested);
                let mid_pass = Rc::clone(&mid_pass);
                raw_navigator(&outer, move || {
                    *mid_pass.borrow_mut() = registrant_ids();
                    if show_nested.get() {
                        auto_wire(&nested);
                        any(raw_navigator(&nested, page))
                    } else {
                        page()
                    }
                })
            })
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        assert!(nested.is_mounted(), "the nested navigator is in the tree");
        assert_eq!(registrant_ids(), vec![outer.id(), nested.id()]);

        // While mounted it survives the outer navigator's double wire — the
        // exact sequence the one-cycle rule alone misreads as a build cycle.
        root.rebuild(&mut app, &mut ());
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            registrant_ids(),
            vec![outer.id(), nested.id()],
            "a mounted navigator is in the tree by definition: never pruned"
        );
        assert_eq!(
            *mid_pass.borrow(),
            vec![outer.id(), nested.id()],
            "and it is never MISSING mid-pass either — the window in which a \
             spurious prune would mis-answer handles_back / mis-route a press"
        );

        // Its page rebuilds without it: the widget tears down.
        show_nested.set(false);
        root.rebuild(&mut app, &mut ());
        assert!(!nested.is_mounted(), "teardown dropped the liveness");

        // Now the one-cycle rule reaches it (the outer wire that spans a whole
        // cycle in which the nested navigator never appeared).
        root.rebuild(&mut app, &mut ());
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            registrant_ids(),
            vec![outer.id()],
            "the departed navigator is released, not kept alive by the veto"
        );
    }
}
