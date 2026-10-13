//! Back-press ⇄ navigator glue (predictive-back + facade auto-wiring): the
//! facade is the only crate seeing both `frust-widgets`'
//! [`NavigatorController`] and `frust-reactive`'s back-press source, mirroring
//! [`router_glue`](crate::router_glue) — `frust-widgets` stays reactive-free,
//! `frust-reactive` navigator-free.
//!
//! # The Android back contract
//!
//! Shell delivers a hardware/gesture back press → framework attempts an async
//! back → framework maintains a "handles back" boolean the shell reads, so a
//! *root-level* press falls through to the platform (activity finish). This
//! module is the framework half: a press is consumed exactly once and routed
//! through [`NavigatorController::request_back`] — which applies the top page's
//! `BackPolicy` (`Pop` pops, `DismissAnimated` fires the overlay's dismiss
//! signal, `Veto` swallows it) — never a bare [`pop`](NavigatorController::pop).
//! "Handles back" reads [`back_interest`](NavigatorController::back_interest),
//! not [`can_pop`](NavigatorController::can_pop), so a dismissable/veto overlay
//! at the *root* claims the press ahead-of-time (predictive-back parity) instead
//! of exiting the app.
//!
//! # Two entry points, one consumption source
//!
//! [`auto_wire`], called from the facade's [`navigator`](crate::navigator)
//! wrapper every rebuild (back handling with ZERO back-specific app code), and
//! the explicit [`BackHandler`]/[`attach_back_handler`], constructed in
//! `Component::init` and driven by [`track`](BackHandler::track) from every
//! `Component::build` (Huddle's shape). Both funnel through one process-wide
//! (UI-thread-affine) [`SHARED`] marker, so a press is consumed **exactly
//! once**: whichever entry point runs first in a rebuild consumes it, the other
//! sees the same already-consumed count and no-ops — never a double-pop.
//!
//! # R44-back: arbitration across more than one navigator
//!
//! An app with a root [`overlay_host`](crate::overlay_host) wires **two**
//! navigators — the host and the navigator inside it — and both want the press.
//! `frust-reactive`'s live-provider slot stays single-registrant (see
//! `frust_reactive::back`); this module owns that registration and multiplexes:
//!
//! > **R44-back — a root overlay host outranks every plain navigator; among
//! > [`Role::Navigator`] peers the innermost (last-wired) ranks first, LIFO —
//! > mirroring Android's `OnBackPressedDispatcher`; a [`BackFallback`]
//! > ([`Role::Fallback`]) ranks after every navigator, so it claims a press
//! > only when no navigator does; a back press goes to the first registrant
//! > reporting interest
//! > ([`back_interest()`](NavigatorController::back_interest)` == true` for a
//! > navigator, the app's interest closure for a fallback).**
//!
//! [`SharedBack::registrants`] is that ordered list, sorted by ([`Role`], wire
//! sequence): build order among hosts, *reversed* among navigators so the
//! innermost wins, build order again among fallbacks. **Rank comes from the
//! call site, not wire timing** — [`auto_wire_overlay_host`] registers
//! [`Role::Host`], [`back_fallback`] registers [`Role::Fallback`], every other
//! entry point [`Role::Navigator`] — because both *when* and *how often* a
//! controller wires
//! genuinely vary: a [`BackHandler`] built in `Component::init` wires before any
//! view exists, and Huddle's documented pattern wires one controller **twice in
//! one build pass** (`state.back.track()` beside
//! `frust::navigator(&controller, …)`), so neither first-wire order nor "one
//! wire per pass" is a premise anything here may rest on. `handles_back`/the
//! live provider answer `any(interest)`, and a consumed press goes to the
//! *first* registrant with interest — so one registrant (or a host with no
//! overlays, `compute_back_interest` `false` at depth 1) behaves exactly as if
//! arbitration did not exist.
//!
//! ## Releasing a navigator that went away
//!
//! A navigator that goes away (a screen with its own nested navigator, popped)
//! must stop claiming presses and release its controller clone. Two rules, in
//! priority order:
//!
//! 1. **Mounted is live.** [`NavigatorController::is_mounted`] is exact (the
//!    navigator's `View::build`/`teardown` write it), so a *mounted* registrant
//!    is in the retained tree by definition and is never pruned, however the
//!    wire sequence looks — the load-bearing half: a root overlay host is
//!    mounted from its own `build`, which runs before its page builder and hence
//!    before the inner navigator wires at all.
//! 2. **The one-cycle rule, for registrants never mounted.** A wire from an
//!    already-registered controller ends a window; an unmounted registrant that
//!    did not wire within it is dropped — the one case rule 1 cannot see (a
//!    `BackHandler` whose navigator view never made it into the tree).
//!
//! **Rule 2 must not be trusted for a mounted navigator**: it infers a build
//! cycle from a repeat wire, and the double-wire-per-pass pattern above breaks
//! that inference (the second wire of one pass looks exactly like the first of
//! the next). Before rule 1, that inference pruned the root overlay host
//! mid-pass and re-created the root-modal double-claim bug.
//!
//! **A [`BackFallback`] obeys the same two rules**, which is why it is
//! view-scoped: its liveness is the mount count of its [`back_fallback`]
//! wrappers (rule 1 — `build` increments it, `teardown` decrements it, as a
//! navigator's own `build`/`teardown` do), and rule 2 removes it once every
//! wrapper is unmounted. An unmounted fallback also stops claiming at once:
//! its interest is gated on that same liveness, so it never claims a press in
//! the window before rule 2 reaches it.
//!
//! **First-wire grace (a fallback only).** [`back_fallback`] wires during view
//! *construction*, before its wrapper's `build` runs — unlike a navigator,
//! whose `build` publishes liveness before anything nested wires. So in the
//! very pass that first mounts a wrapper, a navigator double-wiring later in
//! that pass would see an unbuilt, hence unmounted, fallback and prune it.
//! Rule: **a fallback wired but not yet built, rebuilt or torn down counts as
//! mounted until one of those happens** (each clears the grace; the next
//! [`back_fallback`] call over that handle sets it again). A wrapper is always
//! reconciled in the pass that constructed it, so the grace never outlives that
//! pass in practice; the one residual is a [`back_fallback`] view constructed
//! and then discarded unbuilt, which keeps its fallback live until a wrapper
//! over the same handle is reconciled or the handle is dropped.
//!
//! ## Reachability is upstream of arbitration, by design (R23)
//!
//! Ranking decides who wins *among the claimants*, never who may claim. A
//! navigator input cannot reach — one nested inside a page the outer navigator
//! has covered — reports `back_interest() == false` at the source:
//! `frust-widgets` gates it on the hosting page being in the host navigator's
//! `input_routed_pages()` (rule **R23**: navigator reach follows input routing,
//! exactly; see `frust_widgets::NavigatorController::back_interest`). So nothing
//! here filters on visibility and there is **no second notion of reachability**
//! to keep in sync with the widget's — load-bearing for innermost-first ranking,
//! since an off-screen innermost navigator would otherwise outrank the one the
//! user is actually looking at and swallow every press. Reach says *whether* a
//! navigator is in the running, rank *which* of those gets it.
//!
//! # Timing: the live provider closes the stale window
//!
//! Wiring runs during a rebuild's *build* pass, **before** the navigator's
//! `apply_ops` publishes the new stack depth, so the polled [`set_handles_back`]
//! flag lags a stack change by one frame — and a frame gate skipping the settle
//! frame lets that stale value persist. So the wiring also registers a live
//! provider ([`set_can_pop_provider`]) reading `controller.back_interest()`,
//! which `frust_reactive::handles_back` prefers to the flag: it reads CURRENT
//! interest at press time, after `apply_ops` published it. UI-thread-affine (it
//! reads an `Rc`-backed controller through [`SHARED`]); `frust_reactive::back`'s
//! module docs carry the source-side contract.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use frust_core::{BuildCtx, ChangeFlags, View};
use frust_reactive::{CanPopRegistration, back_presses, set_can_pop_provider, set_handles_back};
use frust_widgets::{NavigatorController, NavigatorId};
use reactive_graph::traits::{Get, GetUntracked};

thread_local! {
    /// The UI-thread monotonic source of [`BackFallback`] ids
    /// ([`RegistrantId::Fallback`]). Kept apart from [`SHARED`] so nothing that
    /// resets the arbitration state can ever hand out an id twice.
    static NEXT_FALLBACK_ID: Cell<u64> = const { Cell::new(0) };
}

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
    /// `NavigatorController` or [`BackFallback`] (`!Send`), exactly like
    /// frust-reactive's live provider slot it feeds.
    static SHARED: RefCell<SharedBack> = const { RefCell::new(SharedBack::new()) };
}

/// What a registrant *is*, which is what R44-back ranks it by — taken from the
/// facade entry point that wired it, never inferred from wire timing.
///
/// `Ord` is the arbitration order: [`Host`](Self::Host) sorts first,
/// [`Fallback`](Self::Fallback) last.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Role {
    /// A root [`overlay_host`](crate::overlay_host). Its overlays are drawn over
    /// every piece of chrome, including any inner navigator, so an open one owns
    /// the back press outright — R44-back's host-outranks-navigator half.
    Host,
    /// Any other wired navigator: [`navigator`](crate::navigator)'s auto-wiring
    /// or an explicit [`BackHandler`].
    Navigator,
    /// A [`BackFallback`] wired by [`back_fallback`]: not a navigator at all,
    /// but an app rule ("back at a tab root returns to the home tab"). Declared
    /// last so it sorts after every navigator **whatever the wire order** — a
    /// fallback claims a press only when no navigator does. Its liveness is its
    /// wrappers' mount count (rule 1), and rule 2 removes it once unmounted (see
    /// the module docs' release rules).
    Fallback,
}

/// Who a [`Registrant`] stands for — the identity a re-wire matches on to
/// refresh an entry in place instead of appending a duplicate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RegistrantId {
    /// A navigator, by its controller's id.
    Navigator(NavigatorId),
    /// A [`BackFallback`], by the id [`NEXT_FALLBACK_ID`] gave it.
    Fallback(u64),
}

/// One wired navigator or fallback in [`SharedBack::registrants`] — the unit
/// R44-back arbitrates over. A navigator's closures hold a controller clone, so
/// a registrant keeps its navigator's op queue alive until it is pruned; a
/// fallback's hold only a weak reference to its [`BackFallback`].
struct Registrant {
    /// Which controller (or fallback) this entry stands for, so a re-wire
    /// refreshes it in place instead of appending a duplicate.
    id: RegistrantId,
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
    /// now? For a fallback, its wrappers' mount count > 0 or the first-wire
    /// grace (see the module docs). A `true` here vetoes the one-cycle prune
    /// (see the module docs' release rules). Only ever reads `Cell`s, so it is
    /// safe to call under the [`SHARED`] borrow.
    mounted: Box<dyn Fn() -> bool>,
    /// `controller.back_interest()` — does this navigator claim the next press?
    /// Already `false` for a navigator whose hosting page input cannot reach
    /// (the R23 gate in `frust-widgets`; see the module docs), which is why
    /// nothing here filters on visibility. For a fallback, the app's interest
    /// closure, gated on the fallback's liveness. An `Rc` so it is cloned out
    /// and called with **no** [`SHARED`] borrow outstanding — a fallback's
    /// probe runs app code.
    interest: Rc<dyn Fn() -> bool>,
    /// `controller.request_back()` — deliver the press to this navigator,
    /// honoring its top page's `BackPolicy` (a fallback's `on_back`). An `Rc`
    /// rather than a `Box` so [`route_back`] can clone it out and call it with
    /// **no** [`SHARED`] borrow outstanding.
    request_back: Rc<dyn Fn()>,
}

impl Registrant {
    /// The sort key R44-back orders the arbitration list by: rank first
    /// ([`Role`], `Host` sorts ahead of every `Navigator`, `Fallback` after
    /// every one), then within `Role::Navigator` **innermost-first** (highest
    /// `birth` — a nested navigator always wires after the navigator hosting
    /// it, so the highest `birth` among `Navigator` peers is the innermost
    /// one); `Role::Host` and `Role::Fallback` peers keep first-wire order
    /// (`birth` ascending). A fallback's rank against a navigator never
    /// depends on wire order: `Role` alone decides it.
    ///
    /// Innermost-first is what lets a nested navigator take the press at all:
    /// ordering every rank by first wire (`birth` ascending) would leave an
    /// outer navigator at depth > 1 — exactly the case where a pushed page hosts
    /// a nested navigator — permanently outranking the nested navigator inside
    /// it, so back would pop the outer stack (discarding the whole page the
    /// nested navigator lives on) instead of reaching in. `Role::Host` peers
    /// keep first-wire order: R44-back only ever requires Host to rank above
    /// every Navigator, never a particular order among multiple hosts.
    fn key(&self) -> (Role, u64) {
        match self.role {
            Role::Host | Role::Fallback => (self.role, self.birth),
            // `u64::MAX - birth`: ascending sort on this puts the HIGHEST
            // birth (innermost, wired last) first among `Navigator` peers —
            // LIFO, mirroring Android's `OnBackPressedDispatcher`.
            Role::Navigator => (self.role, u64::MAX - self.birth),
        }
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
    /// Every wired navigator and fallback, sorted by [`Registrant::key`] —
    /// [`Role::Host`] first (build order among hosts, unchanged), then
    /// [`Role::Navigator`] peers innermost-first (LIFO: last-wired sorts first,
    /// the reverse of build order), then [`Role::Fallback`] peers in build
    /// order, after every navigator — the R44-back arbitration list. Kept
    /// sorted on every wire, so a registrant's position never depends on how
    /// often it wires per pass; an unmounted registrant that misses a build
    /// cycle is pruned (see [`register`]), a fallback included once all its
    /// wrappers are unmounted. The live provider (below) reads through this
    /// list, so a press is decided against the CURRENT registrants' interest.
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

/// Whether **any** wired registrant claims the next back press — the value both
/// the live provider and the polled `handles_back` flag publish.
///
/// The interest probes are CLONED OUT of the list and called with **no**
/// [`SHARED`] borrow outstanding: a navigator's probe is a plain `Cell` read,
/// but a [`BackFallback`]'s runs the app's interest closure, which must be free
/// to read anything — this module included — without a `RefCell` panic.
fn any_interest() -> bool {
    interest_probes().iter().any(|interest| interest())
}

/// Every registrant's interest probe, in arbitration order, cloned out of
/// [`SHARED`] so the caller can run them with no borrow outstanding.
fn interest_probes() -> Vec<Rc<dyn Fn() -> bool>> {
    SHARED.with(|shared| {
        shared
            .borrow()
            .registrants
            .iter()
            .map(|r| Rc::clone(&r.interest))
            .collect()
    })
}

/// Register (or refresh) `controller` in the R44-back arbitration list at
/// `role`'s rank — the controller-flavoured wrapper over [`register`]. Called on
/// every wire from both navigator entry points
/// ([`auto_wire`]/[`auto_wire_overlay_host`] and
/// [`BackHandler::new`]/[`track`](BackHandler::track)).
///
/// # Panics
///
/// As [`register`].
fn refresh_interest<State: 'static>(controller: &NavigatorController<State>, role: Role) {
    let mounted_probe = controller.clone();
    let interest_probe = controller.clone();
    let request_probe = controller.clone();
    register(
        RegistrantId::Navigator(controller.id()),
        role,
        Box::new(move || mounted_probe.is_mounted()),
        Rc::new(move || interest_probe.back_interest()),
        Rc::new(move || request_probe.request_back()),
    );
}

/// Register (or refresh) the registrant `id` in the R44-back arbitration list at
/// `role`'s rank and republish the polled `handles_back` flag, registering the
/// live provider once. The core every entry point funnels through — navigators
/// via [`refresh_interest`], fallbacks via [`back_fallback`].
///
/// Ordering and pruning are the whole mechanism — see the module docs' R44-back
/// section. In short: the entry is (re-)inserted at its ([`Role`], first-wire)
/// sort position, so neither *when* nor *how often* a registrant wires can move
/// it; and a repeat wire drops every **unmounted** registrant that did not wire
/// since this registrant's previous wire, a mounted one being in the tree by
/// definition.
///
/// # Panics
///
/// The first call registers the live provider via [`set_can_pop_provider`],
/// which panics off the UI thread — always the case here (a rebuild / a
/// `Component::init` runs on the UI thread).
fn register(
    id: RegistrantId,
    role: Role,
    mounted: Box<dyn Fn() -> bool>,
    interest: Rc<dyn Fn() -> bool>,
    request_back: Rc<dyn Fn()>,
) {
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
                    // controller ever wired as an overlay host stays one. (A
                    // fallback id only ever wires as `Fallback`, so `min` is a
                    // no-op for it.)
                    shared.registrants[index].role.min(role),
                    shared.registrants[index].birth,
                );
                // A repeat wire closes a window. Any registrant that neither
                // wired within it nor is currently mounted is no longer in the
                // tree — drop it rather than let it keep claiming presses. The
                // mounted probe only reads `Cell`s (a navigator's controller, a
                // fallback's mount count and grace) and never re-enters this
                // module, so calling it under the `SHARED` borrow is safe.
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
            mounted,
            interest,
            request_back,
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
            // registrant joining the list needs no re-registration.
            shared.provider = Some(set_can_pop_provider(Box::new(any_interest)));
        }
    });
    // Refresh the polled fallback flag (the live provider is authoritative; this
    // only keeps the no-provider flag roughly in sync — see the module docs).
    set_handles_back(any_interest());
}

/// Deliver a consumed back press under **R44-back**: to the first registrant in
/// arbitration order (hosts first, then plain navigators innermost-first, then
/// fallbacks) that claims it.
///
/// No visibility/reachability filter here on purpose: `back_interest()` is
/// already gated on the claimant's hosting page being input-routed (see the
/// module docs' R23 note), so a navigator the user cannot reach is never in this
/// list's answer to begin with.
///
/// A press no registrant claims at this point is dropped: delivering it to the
/// navigator whose wire consumed it would not be a no-op (that navigator may be
/// deep yet unreachable, e.g. a hidden `IndexedStack` tab, or torn down). The
/// shell already consumed the press via `handles_back`, so if every interest
/// lapsed between that decision and delivery the press is swallowed, not
/// returned to the platform.
fn route_back() {
    // Both the interest probes and the claimant's `request_back` are CLONED OUT
    // of the list (that is what the `Rc`s are for) and called with NO `SHARED`
    // borrow outstanding. A navigator's are a `Cell` read and an op enqueue, but
    // a fallback's are app closures, and a re-entry into this module under a
    // borrow would panic on the already-borrowed `RefCell`.
    let claimant = SHARED
        .with(|shared| {
            shared
                .borrow()
                .registrants
                .iter()
                .map(|r| (Rc::clone(&r.interest), Rc::clone(&r.request_back)))
                .collect::<Vec<_>>()
        })
        .into_iter()
        .find(|(interest, _)| interest())
        .map(|(_, request_back)| request_back);
    if let Some(request_back) = claimant {
        request_back();
    }
}

/// Consume any new back press against the shared marker, reporting whether
/// this call is the one that consumed it (and so must route it). The tracked
/// read of the back-press counter subscribes THIS rebuild, so a later
/// `push_back_press` wakes it (State & Reactivity: the track-per-rebuild
/// contract).
///
/// The shared marker is the single consumption source (see the module docs): if
/// several entry points run in the same rebuild, the first consumes the press
/// and every later one no-ops on the already-consumed count.
fn consume_press() -> bool {
    // Tracked read — subscribes the rebuild to the back-press counter.
    let count = back_presses().count.get();
    SHARED.with(|shared| {
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
    })
}

/// Consume any new back press ([`consume_press`]), routing it through
/// [`NavigatorController::request_back`] (applies the top page's `BackPolicy`)
/// on the registrant [`route_back`] arbitrates to.
///
/// Under R44-back the press goes to the first registrant claiming it; the
/// wiring controller gets no special delivery, and an unclaimed press is
/// dropped.
fn consume_back() {
    if consume_press() {
        route_back();
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
    consume_back();
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
    consume_back();
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
        consume_back();
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
///     BackHandler, Component, NavigatorController, View, attach_back_handler, navigator,
///     text,
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
///     fn build(&self, state: &mut AppState) -> impl View<AppState> {
///         // Called every rebuild: consumes a new back press (via `request_back`)
///         // and keeps the framework's handles-back interest in sync. The
///         // `frust::navigator` call auto-wires the SAME controller — one press
///         // still pops exactly once (shared consumption source).
///         state.back.track();
///         let controller = state.back.controller();
///         navigator(controller, || text("home"))
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

/// An app-level back rule that claims a press only when **no navigator does** —
/// the Android/Material "back at a tab root returns to the home tab; back at the
/// home tab root exits" convention for per-tab navigator stacks.
///
/// Not a navigator: it ranks after every navigator in the back arbitration
/// (R44-back's fallback rank), whatever the wire order, so a selected tab
/// that can still pop (or a root overlay) always takes the press first.
///
/// Construct once — in `Component::init` or app state — and wrap the subtree it
/// belongs to with [`back_fallback`] from every `Component::build`. Cheaply
/// [`Clone`] (`Rc`-backed); every clone is the same fallback.
///
/// **Liveness is view-scoped.** The fallback claims presses only while at least
/// one [`back_fallback`] wrapper over it is mounted: leaving the screen unmounts
/// the wrapper, the fallback stops claiming at once, and the arbitration list
/// releases it (see the module docs' release rules). Merely holding the handle
/// claims nothing.
///
/// The facade's re-export docs carry a back-to-home-tab example and the
/// accepted residual (a wrapper inside a hidden branch still counts as live).
#[derive(Clone)]
pub struct BackFallback {
    inner: Rc<FallbackInner>,
}

/// The shared state behind a [`BackFallback`] and all its clones.
struct FallbackInner {
    /// This fallback's arbitration identity ([`RegistrantId::Fallback`]).
    id: u64,
    /// How many [`BackFallbackView`] wrappers over this fallback are mounted
    /// right now — `build` increments it, `teardown` decrements it. The
    /// fallback's rule-1 liveness: `> 0` is "in the retained tree".
    mounted: Cell<usize>,
    /// The first-wire grace (see the module docs): set by every
    /// [`back_fallback`] wire, cleared by any wrapper's `build`, `rebuild` or
    /// `teardown`. While set, the fallback counts as mounted even with
    /// `mounted == 0` — it was wired by a wrapper that has not been built yet.
    awaiting_build: Cell<bool>,
    /// The app's "do I claim the next press?" closure.
    interest: Box<dyn Fn() -> bool>,
    /// The app's back action, run when this fallback receives a press.
    on_back: Box<dyn Fn()>,
}

impl FallbackInner {
    /// Rule-1 liveness plus the first-wire grace: a mounted wrapper, or a wire
    /// whose wrapper has not been built yet.
    fn live(&self) -> bool {
        self.mounted.get() > 0 || self.awaiting_build.get()
    }

    /// One wrapper reconciled (built, rebuilt or torn down): the grace ends.
    fn reconciled(&self) {
        self.awaiting_build.set(false);
    }
}

impl BackFallback {
    /// Create a fallback that claims a back press while `interest()` is true
    /// (and a [`back_fallback`] wrapper over it is mounted), running `on_back`
    /// for each press it receives.
    ///
    /// Call once — from `Component::init` or wherever app state is built, on
    /// the UI thread. Seeds the shared consumption marker to the current
    /// back-press count, like [`BackHandler::new`], so a press delivered before
    /// this fallback existed does not fire it. Registers nothing yet: a
    /// fallback joins the arbitration when [`back_fallback`] first wraps a view
    /// with it.
    ///
    /// `interest` is polled whenever the shell asks whether the app handles
    /// back, and before each press is routed, with no back-arbitration borrow
    /// outstanding; keep it a cheap, untracked read (`get_untracked`,
    /// [`NavigatorController::depth`]). `on_back` runs during a rebuild's build
    /// pass, like a navigator's `request_back`.
    pub fn new(interest: impl Fn() -> bool + 'static, on_back: impl Fn() + 'static) -> Self {
        seed_consumed();
        let id = NEXT_FALLBACK_ID.with(|next| {
            let id = next.get();
            next.set(id + 1);
            id
        });
        Self {
            inner: Rc::new(FallbackInner {
                id,
                mounted: Cell::new(0),
                awaiting_build: Cell::new(false),
                interest: Box::new(interest),
                on_back: Box::new(on_back),
            }),
        }
    }

    /// Whether this fallback currently counts as live: a wrapper over it is
    /// mounted, or one was just wired and is awaiting its first build.
    #[cfg(test)]
    fn is_live(&self) -> bool {
        self.inner.live()
    }

    /// Register (or refresh) this fallback in the R44-back arbitration list at
    /// [`Role::Fallback`], and open the first-wire grace (see the module docs).
    /// The probes hold only a [`Weak`](std::rc::Weak) reference, so the
    /// arbitration list never keeps the app's closures alive.
    fn wire(&self) {
        self.inner.awaiting_build.set(true);
        let mounted_probe = Rc::downgrade(&self.inner);
        let interest_probe = Rc::downgrade(&self.inner);
        let request_probe = Rc::downgrade(&self.inner);
        register(
            RegistrantId::Fallback(self.inner.id),
            Role::Fallback,
            Box::new(move || mounted_probe.upgrade().is_some_and(|inner| inner.live())),
            Rc::new(move || {
                interest_probe
                    .upgrade()
                    .is_some_and(|inner| inner.live() && (inner.interest)())
            }),
            Rc::new(move || {
                if let Some(inner) = request_probe.upgrade() {
                    (inner.on_back)();
                }
            }),
        );
    }
}

/// Wrap `child` so `fallback` takes part in back arbitration while this view is
/// mounted — the view-scoped half of [`BackFallback`].
///
/// Call from every `Component::build`, exactly like
/// [`navigator`](crate::navigator): at call time it consumes any new back press
/// (the tracked read that subscribes the rebuild, routed under R44-back, so the
/// fallback receives it only if no navigator claims it) and then registers or
/// refreshes the fallback at the fallback rank. The returned
/// [`BackFallbackView`] is a pass-through: it renders `child` unchanged and
/// counts its own mounts, which is the fallback's liveness. It is a [`View`]
/// for every `State` its child is one for, so the function itself names no
/// `State` and a state-agnostic child (a `text`) needs no annotation.
///
/// The facade's re-export docs carry a back-to-home-tab example.
///
/// # Panics
///
/// Runs during a rebuild's build pass; the first wire registers the live
/// `handles_back` provider, which panics off the UI thread (always the UI
/// thread here).
pub fn back_fallback<V>(fallback: &BackFallback, child: V) -> BackFallbackView<V> {
    if consume_press() {
        route_back();
    }
    fallback.wire();
    BackFallbackView {
        fallback: fallback.clone(),
        child,
    }
}

/// The pass-through view [`back_fallback`] returns: renders its child unchanged
/// (`Element` is the child's own) and counts its own mounts into the
/// [`BackFallback`]'s liveness — `build` increments it before the child builds,
/// `teardown` decrements it after the child is torn down.
pub struct BackFallbackView<V> {
    fallback: BackFallback,
    child: V,
}

impl<State: 'static, V: View<State>> View<State> for BackFallbackView<V> {
    type Element = V::Element;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        // Publish liveness FIRST, like a navigator's `build`: anything wiring
        // from inside the child already sees this fallback as mounted.
        let inner = &self.fallback.inner;
        inner.mounted.set(inner.mounted.get() + 1);
        inner.reconciled();
        self.child.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let (next, old) = (&self.fallback.inner, &prev.fallback.inner);
        if !Rc::ptr_eq(next, old) {
            // The slot now wraps a DIFFERENT fallback: move this mount from the
            // old handle to the new one, so `teardown` (which runs through the
            // latest view) decrements the count it pairs with.
            next.mounted.set(next.mounted.get() + 1);
            old.mounted.set(old.mounted.get().saturating_sub(1));
            old.reconciled();
        }
        next.reconciled();
        self.child.rebuild(&prev.child, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.child.teardown(element, ctx);
        let inner = &self.fallback.inner;
        inner.mounted.set(inner.mounted.get().saturating_sub(1));
        inner.reconciled();
    }

    fn message(&self, element: &mut Self::Element, state: &mut State) {
        self.child.message(element, state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::patterns::SharedAxis;
    use crate::motion::switcher::PatternSwitcherView;
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

    /// The build closure type [`RenderRoot::rebuild`] drives, boxed so
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
        registrants()
            .into_iter()
            .filter_map(|id| match id {
                RegistrantId::Navigator(id) => Some(id),
                RegistrantId::Fallback(_) => None,
            })
            .collect()
    }

    /// The whole arbitration list, fallbacks included, in order.
    fn registrants() -> Vec<RegistrantId> {
        SHARED.with(|shared| shared.borrow().registrants.iter().map(|r| r.id).collect())
    }

    /// **R44-back**: with a root overlay open, the press goes to the HOST even
    /// though the inner navigator is poppable and wired *later*. Without the
    /// rank the last wire would win outright and this press would pop the inner
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
            "and NOT to the inner navigator (the root-modal back bug)"
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
        // is dropped).
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
    ///
    /// All three controllers here register as plain `Role::Navigator` peers —
    /// despite the `outer`/`inner`/`nested` names, nothing wires one *inside*
    /// another — so this test pins only the birth tiebreak among equal-rank
    /// registrants (innermost/last-wire first), not `Role::Host`-first ranking
    /// (that is
    /// `an_init_registered_backhandler_does_not_outrank_a_later_overlay_host`).
    /// The expected order below is therefore the REVERSE of build order, by
    /// design.
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
            vec![nested.id(), inner.id(), outer.id()],
            "innermost/last-wired first: the reverse of build order"
        );

        // Frame 2: the nested navigator's screen is gone — it never wires again.
        refresh_interest(&outer, Role::Navigator);
        refresh_interest(&inner, Role::Navigator);
        assert_eq!(
            registrant_ids(),
            vec![nested.id(), inner.id(), outer.id()],
            "not yet: a full cycle has not elapsed without it"
        );

        // Frame 3: the outer wire now spans a whole cycle in which `nested`
        // never appeared.
        refresh_interest(&outer, Role::Navigator);
        assert_eq!(
            registrant_ids(),
            vec![inner.id(), outer.id()],
            "the departed navigator is pruned, and its controller clone released"
        );

        // Re-wiring an existing registrant never re-orders it.
        refresh_interest(&inner, Role::Navigator);
        refresh_interest(&outer, Role::Navigator);
        refresh_interest(&inner, Role::Navigator);
        assert_eq!(registrant_ids(), vec![inner.id(), outer.id()]);
    }

    // -----------------------------------------------------------------------
    // The two ways a "a repeat wire proves a build cycle" premise breaks — a
    // controller wiring TWICE per pass, and a `BackHandler` wiring from
    // `Component::init` before any view exists.
    // -----------------------------------------------------------------------

    /// The REAL Huddle shape under a root overlay host: the inner navigator is
    /// wired **twice in one build pass** — an explicit `BackHandler` built in
    /// `Component::init` and `track()`ed from the host's page builder, plus the
    /// automatic `navigator()` wiring right beside it
    /// (`examples/huddle/src/lib.rs` does exactly this pair).
    ///
    /// Without the mounted veto, the second wire's one-cycle prune reads the
    /// FIRST wire of the same pass as a cycle boundary and drops the host, which
    /// then re-appends *after* the inner navigator — permanently inverting
    /// arbitration and re-creating the root-modal back bug (a press with a root
    /// modal open popping the page underneath it).
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
            "and NOT the page under it (the root-modal back bug)"
        );
    }

    /// The second ordering break: `BackHandler::new` wires from
    /// `Component::init`, which runs before ANY view is constructed — so a
    /// first-wire-order rule alone would give the inner navigator position 0
    /// from frame 1, and no amount of "track from inside the host's page
    /// builder" advice could fix a registration that already happened. Rank
    /// comes from the entry point instead, so the host still sorts first.
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
    ///
    /// `nested` genuinely lives inside `outer`'s own page here, so it wires
    /// after `outer` and — under innermost-first `Role::Navigator` order — sorts
    /// AHEAD of it; that ordering is incidental to this test's own point
    /// (mount-liveness pruning).
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
        assert_eq!(registrant_ids(), vec![nested.id(), outer.id()]);

        // While mounted it survives the outer navigator's double wire — the
        // exact sequence the one-cycle rule alone misreads as a build cycle.
        root.rebuild(&mut app, &mut ());
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            registrant_ids(),
            vec![nested.id(), outer.id()],
            "a mounted navigator is in the tree by definition: never pruned"
        );
        assert_eq!(
            *mid_pass.borrow(),
            vec![nested.id(), outer.id()],
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

    // -----------------------------------------------------------------------
    // Innermost-first: a nested navigator must receive the back press.
    // -----------------------------------------------------------------------

    /// An outer navigator PUSHES a page that hosts a nested navigator (the outer
    /// navigator is at depth > 1 — exactly what makes `compute_back_interest`
    /// true for it too), and the nested navigator's own stack is also poppable.
    /// A back press must reach the INNERMOST (nested) navigator, popping its
    /// stack, and must leave the outer navigator's depth untouched — the reverse
    /// of what plain build-order (outermost-first) arbitration gives, where the
    /// outer navigator, wired first, claims the press and pops the whole page
    /// hosting the nested navigator instead of reaching into it.
    #[test]
    fn a_nested_navigator_inside_a_pushed_page_gets_the_back_press() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();

        // The outer navigator's ROOT page is plain — no nested navigator
        // exists at depth 1, so `compute_back_interest` starts false for it
        // (matching the module docs: the bug only appears once the outer
        // navigator itself is poppable).
        let mut app: AppLogic = {
            let outer_c = outer.clone();
            Box::new(move |_: &mut ()| {
                auto_wire(&outer_c);
                raw_navigator(&outer_c, page)
            })
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        assert_eq!(outer.depth(), 1, "outer starts at its own root");
        assert!(
            !handles_back(),
            "root-level, no nested navigator yet: back bubbles"
        );

        // Push a page onto the OUTER navigator that hosts the nested
        // navigator — outer is now at depth 2, so it claims back interest too:
        // the shape the ranking has to arbitrate.
        {
            let inner_for_push = inner.clone();
            outer.push(move || {
                auto_wire(&inner_for_push);
                any(raw_navigator(&inner_for_push, page))
            });
        }
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            outer.depth(),
            2,
            "outer pushed to depth 2: the page hosting the nested navigator"
        );
        assert_eq!(
            inner.depth(),
            1,
            "the nested navigator starts at its own root"
        );

        // Push inside the nested navigator too, so BOTH stacks are poppable —
        // the exact ambiguity R44-back must arbitrate between two
        // `Role::Navigator` peers.
        inner.push(page);
        root.rebuild(&mut app, &mut ());
        assert_eq!((outer.depth(), inner.depth()), (2, 2));
        assert!(handles_back(), "some registrant claims the press");

        push_back_press();
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            inner.depth(),
            1,
            "the back press popped the NESTED navigator's stack"
        );
        assert_eq!(
            outer.depth(),
            2,
            "and left the outer navigator's depth UNCHANGED — a bare \
             build-order (outermost-first) arbitration would instead have \
             popped the outer stack, discarding the whole page the nested \
             navigator lives on"
        );
    }

    // -----------------------------------------------------------------------
    // R23, the converse of innermost-first: a nested navigator whose hosting
    // page input can no longer reach must NOT take the press.
    // -----------------------------------------------------------------------

    /// An outer navigator whose ROOT page hosts a nested navigator, each
    /// auto-wired where its view is constructed (the outer from the app's build
    /// pass, the nested from the page builder the outer runs during its own
    /// reconcile) — the canonical tab/section-stack shape. `cull` sets the outer
    /// navigator's [`cull_covered_builds`], the switch that decides whether the
    /// covered page keeps re-running its builder at all.
    struct NestedHarness {
        outer: NavigatorController<()>,
        nested: NavigatorController<()>,
        root: RenderRoot<(), NavigatorView<()>>,
        app: AppLogic,
    }

    impl NestedHarness {
        fn new(cull: bool) -> Self {
            let outer: NavigatorController<()> = NavigatorController::new();
            let nested: NavigatorController<()> = NavigatorController::new();
            let app: AppLogic = {
                let outer = outer.clone();
                let nested = nested.clone();
                Box::new(move |_: &mut ()| {
                    // Exactly what `frust::navigator` does, outermost first.
                    auto_wire(&outer);
                    let nested = nested.clone();
                    raw_navigator(&outer, move || {
                        auto_wire(&nested);
                        any(raw_navigator(&nested, page))
                    })
                    .cull_covered_builds(cull)
                })
            };
            let mut harness = Self {
                outer,
                nested,
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

    /// The outer navigator pushes a page OVER the one hosting the nested
    /// navigator. Input can reach only the outer navigator's top page
    /// (`input_routed_pages`), so back must follow: the press pops the OUTER
    /// stack, and the nested (invisible) stack is untouched.
    ///
    /// Without the R23 reach gate at the source, `route_back`'s
    /// innermost-first `find(|r| interest())` would hand the press to the nested
    /// navigator — the covered page keeps reconciling (the default) or stays
    /// `is_mounted()` (under the cull) either way — popping a stack nobody can
    /// see while the back button appears to do nothing.
    ///
    /// Run under BOTH `cull_covered_builds` settings: they are exactly the two
    /// ways the covered page's builder does or does not keep running, and the
    /// answer must not depend on that.
    #[test]
    fn a_nested_navigator_on_a_covered_page_does_not_take_the_back_press() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        for cull in [false, true] {
            reset();
            let mut h = NestedHarness::new(cull);

            // The nested stack is poppable while its page is still current —
            // the innermost-first case, which must keep working (below too).
            h.nested.push(page);
            h.rebuild();
            assert_eq!((h.outer.depth(), h.nested.depth()), (1, 2));
            assert!(
                handles_back(),
                "the nested navigator claims the press while its page is current (cull={cull})"
            );

            // Now push a page over it on the OUTER navigator: the nested
            // navigator's page is covered and routed no input.
            h.outer.push(page);
            h.rebuild();
            assert_eq!((h.outer.depth(), h.nested.depth()), (2, 2));
            // A few idle frames: under `cull_covered_builds(true)` the covered
            // page stops rebuilding entirely here, so this is where a
            // wire-refreshed (rather than live) reachability flag would go stale.
            for _ in 0..3 {
                h.rebuild();
            }
            assert!(
                handles_back(),
                "the outer navigator claims it (cull={cull})"
            );

            push_back_press();
            h.rebuild();
            assert_eq!(
                h.outer.depth(),
                1,
                "the press popped the OUTER stack — the page the user is \
                 actually looking at came off (cull={cull})"
            );
            assert_eq!(
                h.nested.depth(),
                2,
                "and the covered navigator's invisible stack is UNCHANGED \
                 (cull={cull})"
            );

            // Revealed again, the nested navigator takes the next press — the
            // innermost-first behaviour, unregressed, in the same harness.
            push_back_press();
            h.rebuild();
            assert_eq!(
                (h.outer.depth(), h.nested.depth()),
                (1, 1),
                "back reaches the nested navigator again once its page is \
                 current (cull={cull})"
            );
        }
    }

    /// The reach gate is **input routing**, not painting: a *transparent*
    /// overlay pushed on the outer navigator leaves the page below it painted
    /// (`PageVisibility::Visible`) but routes it no input, so the nested
    /// navigator on that page must not take the press either. This is the same
    /// divergence R23 pins for semantics.
    #[test]
    fn a_nested_navigator_under_a_transparent_overlay_does_not_take_the_back_press() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = NestedHarness::new(false);
        h.nested.push(page);
        h.rebuild();
        h.outer
            .push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
        h.rebuild();
        assert_eq!((h.outer.depth(), h.nested.depth()), (2, 2));

        push_back_press();
        h.rebuild();
        assert_eq!(h.outer.depth(), 1, "the overlay came off");
        assert_eq!(
            h.nested.depth(),
            2,
            "the painted-but-inert page's navigator kept its stack"
        );
    }

    // -----------------------------------------------------------------------
    // A design-claim stress test: wiring-order independence between two
    // `Role::Navigator` peers. `Registrant::key`'s own doc states the
    // premise plainly — "a nested navigator always wires after the navigator
    // hosting it, so the highest `birth` among `Navigator` peers is the
    // innermost one" — and the two tests above this section
    // (`an_overlay_host_survives_an_inner_navigator_wiring_twice_per_pass`,
    // `an_init_registered_backhandler_does_not_outrank_a_later_overlay_host`)
    // already prove that premise does NOT hold for wire order in general
    // (`Role::Host` is decoupled from it on purpose), but only for a `Host`
    // vs. a `Navigator`. Nothing pins the analogous case for two `Navigator`
    // peers — this section does. `refresh_interest`/`Role`/`registrant_ids`
    // are crate-private, so this composition cannot be built from an
    // integration test the way the rest of the shell matrix is.
    // -----------------------------------------------------------------------

    /// `inner` gets an explicit [`BackHandler`], built and FIRST wired before
    /// `outer` (its eventual host) has any view at all — the
    /// `Component::init` shape — and `track()`ed every rebuild from then on,
    /// so its earliest `birth` is never dropped by the one-cycle prune even
    /// while its own navigator widget doesn't exist yet (a bare, never-tracked
    /// pre-registration IS pruned before outer's page ever mounts inner for
    /// real, which self-heals the ordering — tried first, and not what this
    /// test pins). The REST of the composition is the honest structural
    /// nesting the shell matrix uses elsewhere: outer's plain root, then a
    /// page it pushes that hosts `inner`'s real navigator. With BOTH
    /// navigators poppable, the innermost-first rule says `inner` must win.
    ///
    /// **Observed vs. designed:** a `Role::Navigator` peer's sort key is
    /// `u64::MAX - birth`, and `birth` never changes after a registrant's
    /// FIRST wire (only `seq` refreshes) — so pinning `inner`'s birth ahead of
    /// `outer`'s gives `inner` the numerically LARGER key, and `route_back`'s
    /// first-match search finds `outer` first. `outer` wins, inverting the
    /// documented "structurally innermost wins" rule for this ordering. This
    /// is a real, reachable divergence between the documented rule (nesting
    /// depth) and its actual implementation (raw wire sequence), not a test
    /// bug — see the `#[ignore]` reason for the acceptance criterion this
    /// pins for future ranking work.
    #[test]
    #[ignore = "pins a design case that FAILS against current ranking: an inner \
                navigator whose explicit BackHandler wires (and keeps tracking) \
                before its outer host's first view exists (the Component::init \
                shape) loses the innermost-first tie-break to the outer \
                navigator instead of winning it — Registrant::key ranks \
                Role::Navigator peers by raw wire sequence (birth), not by \
                structural nesting depth. Expected to start passing once \
                ranking accounts for nesting depth rather than birth order \
                alone; rerun with `cargo test -p frust-ui --lib \
                wiring_order_does_not_flip -- --ignored` to observe the \
                current failure."]
    fn wiring_order_does_not_flip_the_innermost_navigator_when_it_is_still_poppable() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();

        // `Component::init`: inner's explicit handler wires FIRST, before
        // outer's own view exists.
        let inner_back = BackHandler::new(inner.clone());
        assert_eq!(registrant_ids(), vec![inner.id()], "inner wired alone");

        let mut app: AppLogic = {
            let outer_c = outer.clone();
            Box::new(move |_: &mut ()| {
                // Tracked every pass, exactly like Huddle's own
                // `state.back.track()` — this is what keeps inner's early
                // birth from ever being pruned as stale.
                inner_back.track();
                auto_wire(&outer_c);
                raw_navigator(&outer_c, page)
            })
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        assert_eq!(outer.depth(), 1, "outer starts at its own root");

        {
            let inner_for_push = inner.clone();
            outer.push(move || {
                auto_wire(&inner_for_push);
                any(raw_navigator(&inner_for_push, page))
            });
        }
        root.rebuild(&mut app, &mut ());
        assert_eq!(outer.depth(), 2, "outer pushed the page hosting inner");

        // The case-5 shape: BOTH navigators poppable.
        inner.push(page);
        root.rebuild(&mut app, &mut ());
        assert_eq!((outer.depth(), inner.depth()), (2, 2));

        push_back_press();
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            inner.depth(),
            1,
            "the structurally-innermost navigator should win the tie \
             regardless of which one registered first"
        );
        assert_eq!(
            outer.depth(),
            2,
            "and the outer stack should stay untouched"
        );
    }

    /// The harmless half of the same stress test: at inner depth 1 (default
    /// `BackPolicy::Pop`, no interest either way), wire order cannot matter
    /// because ranking is never even consulted — `route_back` only compares
    /// registrants that both claim interest, and `inner` claims none here.
    /// Unlike its sibling above, this one is expected to (and does) pass
    /// against unchanged code.
    #[test]
    fn wiring_order_is_irrelevant_when_the_inner_navigator_has_no_interest() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();

        let inner_back = BackHandler::new(inner.clone());

        let mut app: AppLogic = {
            let outer_c = outer.clone();
            Box::new(move |_: &mut ()| {
                inner_back.track();
                auto_wire(&outer_c);
                raw_navigator(&outer_c, page)
            })
        };
        let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());

        {
            let inner_for_push = inner.clone();
            outer.push(move || {
                auto_wire(&inner_for_push);
                any(raw_navigator(&inner_for_push, page))
            });
        }
        root.rebuild(&mut app, &mut ());
        assert_eq!(
            (outer.depth(), inner.depth()),
            (2, 1),
            "outer poppable, inner still at its own root"
        );

        push_back_press();
        root.rebuild(&mut app, &mut ());
        assert_eq!(outer.depth(), 1, "outer popped its own shell page");
    }

    // -----------------------------------------------------------------------
    // Per-tab navigators kept mounted under an `IndexedStack`: only the
    // selected tab may claim the back press, whatever the wire order.
    // -----------------------------------------------------------------------

    type TabsApp = Box<dyn FnMut(&mut ()) -> AnyView<()>>;

    /// Two tab navigators inside `IndexedStack(selected, (tab0, tab1))`,
    /// optionally under a root `overlay_host`. `last_wired_is_tab1` picks
    /// which tab's `crate::navigator` call (and so its wiring) runs last.
    struct TabsHarness {
        tab0: NavigatorController<()>,
        tab1: NavigatorController<()>,
        host: NavigatorController<()>,
        selected: std::rc::Rc<std::cell::Cell<Option<usize>>>,
        root: RenderRoot<(), AnyView<()>>,
        app: TabsApp,
    }

    impl TabsHarness {
        fn new(selected: usize, last_wired_is_tab1: bool, with_host: bool) -> Self {
            Self::build(selected, last_wired_is_tab1, with_host, |_, _, _| None)
        }

        /// As [`new`](Self::new) (no host, tab 1 wired last), with the tabs
        /// wrapped in `back_fallback` over the fallback `make_fallback` builds
        /// from the two tab controllers and the selection cell.
        fn with_fallback(
            selected: usize,
            make_fallback: impl FnOnce(
                &NavigatorController<()>,
                &NavigatorController<()>,
                &std::rc::Rc<std::cell::Cell<Option<usize>>>,
            ) -> BackFallback,
        ) -> Self {
            Self::build(selected, true, false, |tab0, tab1, sel| {
                Some(make_fallback(tab0, tab1, sel))
            })
        }

        fn build(
            selected: usize,
            last_wired_is_tab1: bool,
            with_host: bool,
            make_fallback: impl FnOnce(
                &NavigatorController<()>,
                &NavigatorController<()>,
                &std::rc::Rc<std::cell::Cell<Option<usize>>>,
            ) -> Option<BackFallback>,
        ) -> Self {
            let tab0: NavigatorController<()> = NavigatorController::new();
            let tab1: NavigatorController<()> = NavigatorController::new();
            let host: NavigatorController<()> = NavigatorController::new();
            let sel = std::rc::Rc::new(std::cell::Cell::new(Some(selected)));
            let fallback = make_fallback(&tab0, &tab1, &sel);
            let app: TabsApp = {
                let (tab0, tab1, host, sel) =
                    (tab0.clone(), tab1.clone(), host.clone(), sel.clone());
                Box::new(move |_: &mut ()| {
                    let tabs = {
                        let (tab0, tab1, sel) = (tab0.clone(), tab1.clone(), sel.clone());
                        move || {
                            let (n0, n1);
                            if last_wired_is_tab1 {
                                n0 = crate::navigator(&tab0, page);
                                n1 = crate::navigator(&tab1, page);
                            } else {
                                n1 = crate::navigator(&tab1, page);
                                n0 = crate::navigator(&tab0, page);
                            }
                            frust_widgets::IndexedStack(sel.get(), (n0, n1))
                        }
                    };
                    if with_host {
                        auto_wire_overlay_host(&host);
                        any(raw_overlay_host(&host, move || any(tabs())))
                    } else if let Some(fallback) = &fallback {
                        any(back_fallback(fallback, tabs()))
                    } else {
                        any(tabs())
                    }
                })
            };
            let mut h = Self {
                tab0,
                tab1,
                host,
                selected: sel,
                root: RenderRoot::new(),
                app,
            };
            h.rebuild();
            h
        }

        fn rebuild(&mut self) {
            self.root.rebuild(&mut self.app, &mut ());
        }

        fn select(&mut self, i: usize) {
            self.selected.set(Some(i));
            self.rebuild();
        }

        fn depths(&self) -> (usize, usize) {
            (self.tab0.depth(), self.tab1.depth())
        }

        /// Tab 0 to depth `d0`, tab 1 to depth `d1`.
        fn deepen(&mut self, d0: usize, d1: usize) {
            // Depth only updates on rebuild, so push by count from the
            // current (settled) depth.
            for _ in self.tab0.depth()..d0 {
                self.tab0.push(page);
            }
            for _ in self.tab1.depth()..d1 {
                self.tab1.push(page);
            }
            self.rebuild();
            assert_eq!(self.depths(), (d0, d1));
        }
    }

    /// (a) Selected tab 0 at its root: back bubbles to the platform even though
    /// the hidden tab 1 (wired last) is deep.
    #[test]
    fn a_selected_tab_at_its_root_exits_even_if_a_hidden_tab_is_deep() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = TabsHarness::new(0, true, false);
        h.deepen(1, 3);
        assert!(
            !handles_back(),
            "selected tab at its root: the app would exit"
        );

        push_back_press();
        h.rebuild();
        assert_eq!(h.depths(), (1, 3), "the hidden tab was not popped");
    }

    /// (b) Switching to tab 1 hands it the press; tab 0 is untouched.
    #[test]
    fn switching_tabs_hands_the_back_press_to_the_newly_selected_tab() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = TabsHarness::new(0, true, false);
        h.deepen(1, 3);
        h.select(1);
        assert!(handles_back(), "selected tab 1 is poppable");

        push_back_press();
        h.rebuild();
        assert_eq!(h.depths(), (1, 2), "tab 1 popped one page, tab 0 untouched");
    }

    /// (c) Wire-order independence, both orders: the deep NON-selected tab is
    /// wired last, yet the press goes to the selected tab.
    #[test]
    fn the_back_press_follows_selection_not_wire_order() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        for last_wired_is_tab1 in [true, false] {
            reset();
            // Selected tab 0 (depth 2) vs hidden tab 1 (depth 3).
            let mut h = TabsHarness::new(0, last_wired_is_tab1, false);
            h.deepen(2, 3);
            assert!(handles_back());
            push_back_press();
            h.rebuild();
            assert_eq!(
                h.depths(),
                (1, 3),
                "selected tab 0 took the press (tab1 wired last: {last_wired_is_tab1})"
            );

            // And the mirror: select tab 1, which now wins whichever wired last.
            h.select(1);
            h.tab0.push(page);
            h.rebuild();
            assert_eq!(h.depths(), (2, 3));
            push_back_press();
            h.rebuild();
            assert_eq!(
                h.depths(),
                (2, 2),
                "selected tab 1 took the press (tab1 wired last: {last_wired_is_tab1})"
            );
        }
    }

    /// (d) A root overlay host with an open overlay still outranks the
    /// selected tab.
    #[test]
    fn a_root_overlay_outranks_the_selected_tab() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = TabsHarness::new(1, false, true);
        h.deepen(1, 2);
        h.host
            .push_with_options(page, PushOptions::transparent().back(BackPolicy::Pop));
        h.rebuild();
        assert_eq!(h.host.depth(), 2);
        assert!(handles_back());

        push_back_press();
        h.rebuild();
        assert_eq!(h.host.depth(), 1, "the overlay was dismissed first");
        assert_eq!(h.depths(), (1, 2), "the selected tab was untouched");

        push_back_press();
        h.rebuild();
        assert_eq!(
            h.depths(),
            (1, 1),
            "the next press reaches the selected tab"
        );
    }

    /// (e) Hidden tabs stay mounted and registered: depth survives a round
    /// trip with no re-push.
    #[test]
    fn a_hidden_tab_keeps_its_stack_and_registration() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = TabsHarness::new(0, true, false);
        h.deepen(1, 3);
        h.select(1);
        h.select(0);
        for _ in 0..3 {
            h.rebuild();
        }
        assert_eq!(h.depths(), (1, 3), "the hidden tab's depth survived");
        let ids = registrant_ids();
        assert!(
            ids.contains(&h.tab0.id()) && ids.contains(&h.tab1.id()),
            "both tab controllers stay registered while hidden"
        );

        h.select(1);
        assert!(handles_back());
        push_back_press();
        h.rebuild();
        assert_eq!(h.depths(), (1, 2), "tab 1 resumes from its kept stack");
    }

    // -----------------------------------------------------------------------
    // `BackFallback`: a non-navigator registrant ranked after every navigator,
    // live while a `back_fallback` wrapper over it is mounted.
    // -----------------------------------------------------------------------

    /// A `RenderRoot` driving a concretely typed app closure — no erasure
    /// needed when the root view's type is fixed.
    struct Driver<V: View<()>, F: FnMut(&mut ()) -> V> {
        root: RenderRoot<(), V>,
        app: F,
    }

    impl<V: View<()>, F: FnMut(&mut ()) -> V> Driver<V, F> {
        fn new(app: F) -> Self {
            let mut driver = Self {
                root: RenderRoot::new(),
                app,
            };
            driver.rebuild();
            driver
        }

        fn rebuild(&mut self) {
            self.root.rebuild(&mut self.app, &mut ());
        }
    }

    /// An `on_back` that counts its calls, and the count.
    fn counting() -> (Rc<Cell<usize>>, impl Fn() + 'static) {
        let hits = Rc::new(Cell::new(0));
        let counter = Rc::clone(&hits);
        (hits, move || counter.set(counter.get() + 1))
    }

    /// `fallback`'s arbitration identity.
    fn fallback_id(fallback: &BackFallback) -> RegistrantId {
        RegistrantId::Fallback(fallback.inner.id)
    }

    /// Whether `fallback` is in the arbitration list at all.
    fn is_registered(fallback: &BackFallback) -> bool {
        registrants().contains(&fallback_id(fallback))
    }

    /// The root view [`wrapped_navigator`] drives, and its driver.
    type WrappedNavigator = BackFallbackView<NavigatorView<()>>;
    type WrappedDriver = Driver<WrappedNavigator, Box<dyn FnMut(&mut ()) -> WrappedNavigator>>;

    /// A navigator (at its root) wrapped by `back_fallback`, plus the fallback
    /// and its call count, with the fallback's interest fixed to `interest`.
    fn wrapped_navigator(
        interest: bool,
    ) -> (
        NavigatorController<()>,
        BackFallback,
        Rc<Cell<usize>>,
        WrappedDriver,
    ) {
        let nav: NavigatorController<()> = NavigatorController::new();
        let (hits, on_back) = counting();
        let fallback = BackFallback::new(move || interest, on_back);
        let driver = {
            let (nav, fallback) = (nav.clone(), fallback.clone());
            Driver::new(Box::new(move |_: &mut ()| {
                back_fallback(&fallback, crate::navigator(&nav, page))
            })
                as Box<dyn FnMut(&mut ()) -> WrappedNavigator>)
        };
        (nav, fallback, hits, driver)
    }

    /// (a) A navigator that can pop outranks a mounted, interested fallback.
    #[test]
    fn a_poppable_navigator_outranks_a_fallback() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let (nav, _fallback, hits, mut d) = wrapped_navigator(true);
        nav.push(page);
        d.rebuild();
        assert_eq!(nav.depth(), 2);

        push_back_press();
        d.rebuild();
        assert_eq!(nav.depth(), 1, "the navigator took the press");
        assert_eq!(hits.get(), 0, "and the fallback did not");
    }

    /// (b) With the navigator at its root, an interested fallback claims the
    /// press: `handles_back()` is true, `on_back` runs once, and the navigator
    /// is untouched.
    #[test]
    fn a_fallback_claims_the_press_when_no_navigator_does() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let (nav, _fallback, hits, mut d) = wrapped_navigator(true);
        assert_eq!(nav.depth(), 1);
        assert!(
            handles_back(),
            "the fallback claims the press ahead of time"
        );

        push_back_press();
        d.rebuild();
        assert_eq!(hits.get(), 1, "on_back ran once");
        assert_eq!(nav.depth(), 1, "the navigator is unchanged");

        d.rebuild();
        assert_eq!(hits.get(), 1, "a rebuild with no new press does not refire");
    }

    /// (c) A fallback with no interest lets a root-level press exit.
    #[test]
    fn an_uninterested_fallback_lets_the_press_exit() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let (nav, _fallback, hits, mut d) = wrapped_navigator(false);
        assert!(!handles_back(), "nobody claims: the app would exit");

        push_back_press();
        d.rebuild();
        assert_eq!((hits.get(), nav.depth()), (0, 1));
    }

    /// (d) Rank comes from `Role`, never from wire order: a fallback wired
    /// BEFORE the navigator and one wired AFTER it both rank after it, and
    /// among themselves keep first-wire order.
    #[test]
    fn fallbacks_rank_after_the_navigator_whatever_the_wire_order() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let nav: NavigatorController<()> = NavigatorController::new();
        let (before_hits, before_on_back) = counting();
        let (after_hits, after_on_back) = counting();
        let before = BackFallback::new(|| true, before_on_back);
        let after = BackFallback::new(|| true, after_on_back);
        let mut d = {
            let (nav, before, after) = (nav.clone(), before.clone(), after.clone());
            Driver::new(move |_: &mut ()| {
                // Argument order is wire order: `before`, the navigator, `after`.
                frust_widgets::stack()
                    .child(back_fallback(&before, frust_widgets::text("x")))
                    .child(back_fallback(&after, crate::navigator(&nav, page)))
            })
        };
        assert_eq!(
            registrants(),
            vec![
                RegistrantId::Navigator(nav.id()),
                fallback_id(&before),
                fallback_id(&after),
            ],
            "the navigator first, then the fallbacks in first-wire order"
        );

        nav.push(page);
        d.rebuild();
        push_back_press();
        d.rebuild();
        assert_eq!(nav.depth(), 1, "the navigator took the press");
        assert_eq!((before_hits.get(), after_hits.get()), (0, 0));

        push_back_press();
        d.rebuild();
        assert_eq!(
            (before_hits.get(), after_hits.get()),
            (1, 0),
            "with the navigator at its root, the first-wired fallback took it"
        );
        assert_eq!(
            registrants(),
            vec![
                RegistrantId::Navigator(nav.id()),
                fallback_id(&before),
                fallback_id(&after),
            ],
            "re-wiring never re-orders"
        );
    }

    /// (e) Leaving the screen: the wrapper unmounts, the fallback stops
    /// claiming at once, rule 2 prunes it, and a press does not reach it.
    /// Re-adding the wrapper re-registers it.
    #[test]
    fn an_unmounted_fallback_stops_claiming_and_is_pruned() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let nav: NavigatorController<()> = NavigatorController::new();
        let (hits, on_back) = counting();
        let fallback = BackFallback::new(|| true, on_back);
        let show = Rc::new(Cell::new(true));
        let mut d = {
            let (nav, fallback, show) = (nav.clone(), fallback.clone(), Rc::clone(&show));
            Driver::new(move |_: &mut ()| {
                frust_widgets::stack()
                    .child(crate::navigator(&nav, page))
                    .when(show.get(), |s| {
                        s.child(back_fallback(&fallback, frust_widgets::text("x")))
                    })
            })
        };
        assert!(handles_back(), "mounted and interested");

        show.set(false);
        d.rebuild();
        assert!(!fallback.is_live(), "the wrapper was torn down");
        assert!(!handles_back(), "an unmounted fallback claims nothing");
        d.rebuild();
        d.rebuild();
        assert!(!is_registered(&fallback), "rule 2 pruned it");
        assert_eq!(registrant_ids(), vec![nav.id()]);

        push_back_press();
        d.rebuild();
        assert_eq!(hits.get(), 0, "a press after the prune does not reach it");

        show.set(true);
        d.rebuild();
        assert!(
            is_registered(&fallback),
            "re-adding the wrapper re-registers it"
        );
        assert!(handles_back());
        push_back_press();
        d.rebuild();
        assert_eq!(hits.get(), 1, "and it claims presses again");
    }

    /// (f) A burst of presses before a rebuild collapses to one `on_back`.
    #[test]
    fn a_press_burst_calls_on_back_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let (_nav, _fallback, hits, mut d) = wrapped_navigator(true);
        for _ in 0..3 {
            push_back_press();
        }
        d.rebuild();
        assert_eq!(hits.get(), 1, "three presses, one rebuild: one on_back");
        d.rebuild();
        assert_eq!(hits.get(), 1);
    }

    /// (g) Per-tab stacks under an `IndexedStack`, wrapped by a back-to-home
    /// fallback: back at a non-home tab root switches to the home tab, and
    /// back at the home tab root is not consumed (the app exits).
    #[test]
    fn back_at_a_tab_root_returns_home_then_exits() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        const HOME: usize = 0;
        let mut h = TabsHarness::with_fallback(1, |tab0, tab1, sel| {
            let (tab0, tab1) = (tab0.clone(), tab1.clone());
            let (probe, set) = (Rc::clone(sel), Rc::clone(sel));
            BackFallback::new(
                move || match probe.get() {
                    Some(HOME) | None => false,
                    Some(i) => [&tab0, &tab1][i].depth() <= 1,
                },
                move || set.set(Some(HOME)),
            )
        });

        // A poppable selected tab still takes the press first.
        h.deepen(1, 2);
        push_back_press();
        h.rebuild();
        assert_eq!(h.depths(), (1, 1), "tab B popped");
        assert_eq!(h.selected.get(), Some(1), "and the selection stayed");

        // Tab B at its root: the fallback switches to the home tab.
        assert!(
            handles_back(),
            "off home at a tab root: the fallback claims"
        );
        push_back_press();
        h.rebuild();
        assert_eq!(
            h.selected.get(),
            Some(HOME),
            "the press selected the home tab"
        );
        h.rebuild();
        assert_eq!(h.depths(), (1, 1), "no stack was popped");

        // Home tab at its root: nobody claims, so the press would exit.
        assert!(!handles_back(), "home at its root: back falls through");
        push_back_press();
        h.rebuild();
        assert_eq!(h.selected.get(), Some(HOME));
        assert_eq!(h.depths(), (1, 1));
    }

    /// (h) A navigator that wires twice per pass (`BackHandler::track` +
    /// `navigator()` on one controller) never prunes a mounted fallback: rule 1
    /// holds for a fallback exactly as for a navigator.
    #[test]
    fn a_mounted_fallback_survives_a_double_wiring_navigator() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let nav: NavigatorController<()> = NavigatorController::new();
        let back = BackHandler::new(nav.clone());
        let (hits, on_back) = counting();
        let fallback = BackFallback::new(|| true, on_back);
        let mut d = {
            let (nav, fallback) = (nav.clone(), fallback.clone());
            Driver::new(move |_: &mut ()| {
                back.track();
                back_fallback(&fallback, crate::navigator(&nav, page))
            })
        };
        for pass in 0..4 {
            d.rebuild();
            assert!(is_registered(&fallback), "never pruned (pass {pass})");
            assert!(handles_back(), "handles_back stays true (pass {pass})");
        }

        push_back_press();
        d.rebuild();
        assert_eq!(
            (hits.get(), nav.depth()),
            (1, 1),
            "the fallback took it once"
        );
    }

    /// (i) First-wire grace: `back_fallback()` wires before its wrapper is
    /// built, and a double-wiring navigator constructed AFTER it in the same
    /// first pass must not prune it while its mount count is still 0.
    #[test]
    fn first_wire_grace_keeps_an_unbuilt_fallback_through_a_double_wire() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let nav: NavigatorController<()> = NavigatorController::new();
        let back = BackHandler::new(nav.clone());
        let fallback = BackFallback::new(|| true, || {});
        // What the double wire left behind, seen mid-pass: (registered, mount
        // count) right after the navigator's second wire, before any build.
        let mid_pass = Rc::new(Cell::new((false, usize::MAX)));
        let d = {
            let (nav, fallback, mid_pass) = (nav.clone(), fallback.clone(), Rc::clone(&mid_pass));
            Driver::new(move |_: &mut ()| {
                let wrapped = back_fallback(&fallback, frust_widgets::text("x"));
                back.track();
                let navigator = crate::navigator(&nav, page);
                mid_pass.set((is_registered(&fallback), fallback.inner.mounted.get()));
                frust_widgets::stack().child(wrapped).child(navigator)
            })
        };
        assert_eq!(
            mid_pass.get(),
            (true, 0),
            "unbuilt (mount count 0) yet not pruned by the navigator's double wire"
        );
        assert!(is_registered(&fallback), "still registered after the pass");
        assert_eq!(fallback.inner.mounted.get(), 1, "and now mounted");
        assert!(
            handles_back(),
            "handles_back reflects it on the first frame"
        );
        drop(d);
    }

    /// (j) Two wrappers sharing one handle: the fallback stays live while
    /// either is mounted, and is pruned only once both are gone.
    #[test]
    fn a_fallback_shared_by_two_wrappers_lives_until_both_unmount() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let nav: NavigatorController<()> = NavigatorController::new();
        let fallback = BackFallback::new(|| true, || {});
        let show = Rc::new(Cell::new((true, true)));
        let mut d = {
            let (nav, fallback, show) = (nav.clone(), fallback.clone(), Rc::clone(&show));
            Driver::new(move |_: &mut ()| {
                let (a, b) = show.get();
                frust_widgets::stack()
                    .child(crate::navigator(&nav, page))
                    .when(a, |s| {
                        s.child(back_fallback(&fallback, frust_widgets::text("a")))
                    })
                    .when(b, |s| {
                        s.child(back_fallback(&fallback, frust_widgets::text("b")))
                    })
            })
        };
        assert_eq!(fallback.inner.mounted.get(), 2);

        show.set((true, false));
        for _ in 0..3 {
            d.rebuild();
        }
        assert_eq!(fallback.inner.mounted.get(), 1);
        assert!(is_registered(&fallback), "one wrapper still mounted: live");
        assert!(handles_back());

        show.set((false, false));
        d.rebuild();
        assert!(!handles_back(), "both unmounted: claims nothing");
        d.rebuild();
        d.rebuild();
        assert!(!is_registered(&fallback), "and rule 2 pruned it");
    }

    /// A slot that rebuilds against a DIFFERENT handle moves its mount: the
    /// old fallback unmounts (and is pruned), the new one is live.
    #[test]
    fn a_wrapper_rebuilt_against_another_handle_moves_its_mount() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let nav: NavigatorController<()> = NavigatorController::new();
        let first = BackFallback::new(|| true, || {});
        let second = BackFallback::new(|| true, || {});
        let use_first = Rc::new(Cell::new(true));
        let mut d = {
            let (nav, first, second, use_first) = (
                nav.clone(),
                first.clone(),
                second.clone(),
                Rc::clone(&use_first),
            );
            Driver::new(move |_: &mut ()| {
                let fallback = if use_first.get() { &first } else { &second };
                back_fallback(fallback, crate::navigator(&nav, page))
            })
        };
        assert_eq!(
            (first.inner.mounted.get(), second.inner.mounted.get()),
            (1, 0)
        );

        use_first.set(false);
        for _ in 0..3 {
            d.rebuild();
        }
        assert_eq!(
            (first.inner.mounted.get(), second.inner.mounted.get()),
            (0, 1)
        );
        assert!(!is_registered(&first), "the old handle was released");
        assert!(is_registered(&second), "the new one is live");
    }

    // -----------------------------------------------------------------------
    // Unclaimed presses are dropped; tab navigators + fallback under a real
    // `pattern_switcher`.
    // -----------------------------------------------------------------------

    /// (b) The opposite wire order of the first tabs test: the hidden deep tab
    /// wires FIRST, the selected tab (wired last) is at its root, no fallback.
    /// Nothing claims, and nothing pops.
    #[test]
    fn an_unclaimed_press_pops_nothing_when_the_hidden_deep_tab_wires_first() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = TabsHarness::new(0, false, false);
        h.deepen(1, 3);
        assert!(!handles_back(), "selected tab at its root: nothing claims");

        push_back_press();
        h.rebuild();
        assert_eq!(h.depths(), (1, 3), "the hidden deep tab was not popped");
        h.rebuild();
        assert_eq!(h.depths(), (1, 3));
    }

    /// (c) Lost-claim race: the fallback claims at poll time, the selection
    /// flips to Home before the rebuild that consumes the press, and the
    /// hidden Home tab is deep. The press pops nothing.
    #[test]
    fn a_claim_lost_before_the_press_is_consumed_pops_no_hidden_tab() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        const HOME: usize = 0;
        let mut h = TabsHarness::with_fallback(1, |tab0, tab1, sel| {
            let (tab0, tab1) = (tab0.clone(), tab1.clone());
            let probe = Rc::clone(sel);
            BackFallback::new(
                move || match probe.get() {
                    Some(HOME) | None => false,
                    Some(i) => [&tab0, &tab1][i].depth() <= 1,
                },
                || {},
            )
        });
        h.deepen(3, 1);
        assert!(handles_back(), "tab B at its root: the fallback claims");

        // The selection flips before the rebuild that consumes the press.
        h.selected.set(Some(HOME));
        push_back_press();
        h.rebuild();
        assert_eq!(h.depths(), (3, 1), "the hidden-then-selected Home tab held");
        h.rebuild();
        assert_eq!(h.depths(), (3, 1));
    }

    type SwitchedApp = Box<dyn FnMut(&mut ()) -> PatternSwitcherView<(), SharedAxis>>;

    /// Two tab navigators (`IndexedStack`, optionally under a `back_fallback`)
    /// inside the page of a real `frust::motion` `pattern_switcher` keyed 0;
    /// key 1 is a page with no navigator.
    struct SwitchHarness {
        tab0: NavigatorController<()>,
        tab1: NavigatorController<()>,
        key: Rc<Cell<usize>>,
        hits: Rc<Cell<usize>>,
        driver: Driver<PatternSwitcherView<(), SharedAxis>, SwitchedApp>,
    }

    impl SwitchHarness {
        fn new(with_fallback: bool) -> Self {
            let tab0: NavigatorController<()> = NavigatorController::new();
            let tab1: NavigatorController<()> = NavigatorController::new();
            let key = Rc::new(Cell::new(0usize));
            let (hits, on_back) = counting();
            let fallback = {
                let tab1 = tab1.clone();
                BackFallback::new(move || tab1.depth() <= 1, on_back)
            };
            let app: SwitchedApp = {
                let (tab0, tab1, key) = (tab0.clone(), tab1.clone(), key.clone());
                Box::new(move |_: &mut ()| {
                    let child: AnyView<()> = if key.get() == 0 {
                        let n0 = crate::navigator(&tab0, page);
                        let n1 = crate::navigator(&tab1, page);
                        let tabs = frust_widgets::IndexedStack(Some(1), (n0, n1));
                        if with_fallback {
                            any(back_fallback(&fallback, tabs))
                        } else {
                            any(tabs)
                        }
                    } else {
                        any(frust_widgets::text("no navigator here"))
                    };
                    crate::motion::switcher::pattern_switcher(key.get(), SharedAxis::X, child)
                })
            };
            Self {
                tab0,
                tab1,
                key,
                hits,
                driver: Driver::new(app),
            }
        }

        /// Tab B (the selected one) to `depth`, by a counted number of pushes.
        fn deepen_b(&mut self, depth: usize) {
            for _ in self.tab1.depth()..depth {
                self.tab1.push(page);
            }
            self.driver.rebuild();
            assert_eq!(self.tab1.depth(), depth);
        }

        fn switch_away(&mut self) {
            self.key.set(1);
            self.driver.rebuild();
        }
    }

    /// (a) The real switcher path, deep tab: switching to a page with no
    /// navigator tears the tabs down on the switch rebuild itself, so nothing
    /// claims and the press is not swallowed by the dead tab B.
    #[test]
    fn a_switcher_teardown_leaves_no_claim_on_a_deep_dead_tab() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = SwitchHarness::new(true);
        h.deepen_b(3);
        assert!(
            handles_back(),
            "deep selected tab B claims before the switch"
        );

        h.switch_away();
        assert!(!handles_back(), "no claim on the switch rebuild itself");

        push_back_press();
        h.driver.rebuild();
        assert_eq!(h.tab1.depth(), 3, "the dead tab B was not popped");
        assert_eq!(h.tab0.depth(), 1);
        assert_eq!(h.hits.get(), 0, "the fallback did not fire either");
        assert!(!handles_back());
    }

    /// (a, cont.) The real switcher path, tab B at its root with the fallback
    /// interested: after the switch the fallback is unmounted and does not fire.
    #[test]
    fn a_switcher_teardown_silences_an_interested_fallback() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = SwitchHarness::new(true);
        assert!(handles_back(), "tab B at its root: the fallback claims");

        h.switch_away();
        assert!(!handles_back(), "the torn-down fallback stops claiming");

        push_back_press();
        h.driver.rebuild();
        assert_eq!(h.hits.get(), 0, "the fallback did not fire");
    }

    /// (d) A torn-down navigator stops claiming immediately: no extra rebuild
    /// cycle is needed before `handles_back` turns false.
    #[test]
    fn a_torn_down_navigator_stops_claiming_without_an_extra_rebuild() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));
        reset();

        let mut h = SwitchHarness::new(false);
        h.deepen_b(3);
        assert!(h.tab1.is_mounted());
        assert!(handles_back());

        h.switch_away();
        assert!(!h.tab1.is_mounted(), "torn down at the switch");
        assert!(
            !handles_back(),
            "and no longer claiming, with no extra pass"
        );
    }
}
