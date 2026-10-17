//! The render root: the object each platform shell drives each frame.
//!
//! It owns the widget [`WidgetTree`] and the previous [`View`], and exposes the
//! three framework passes in Masonry order (the subset relevant to v0):
//!
//! * [`RenderRoot::rebuild`] — run the root build closure, diff against the previous view,
//!   producing/mutating the retained widget.
//! * [`RenderRoot::layout`] — hand the root widget window-sized constraints and
//!   record the size it returns.
//! * [`RenderRoot::paint`] — emit the root widget's draw commands into a scene.
//!
//! v0 is single-root: the root component's build closure returns one `impl View<State>` whose concrete
//! type is fixed, so the root's previous view and element are stored typed.
//! ViewSequence / multiple children are explicitly out of scope for now.

mod config;
mod dispatch;
mod layout;
mod metrics;
mod paint;
mod rebuild;
mod semantics;
mod session;
#[cfg(test)]
mod tests;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::num::NonZeroU64;
use std::rc::Weak;
use std::sync::atomic::{AtomicU64, Ordering};

use kurbo::{Rect, Size};

use crate::event::{CursorIcon, EventOutcome, ImeState, PointerId};
use crate::insets::WindowInsets;
use crate::overlay::{OverlayHit, OverlayKey};
use crate::selection_toolbar::SelectionToolbarRequest;
use crate::tree::WidgetTree;
use crate::view::{ChangeFlags, View, WidgetId};
use crate::widget::{ChildPod, PlatformViewFrame};

use layout::LayoutAudit;
pub use metrics::{Orientation, WindowMetrics};

/// How many [`InputEvent::Housekeeping`](crate::event::InputEvent::Housekeeping) flush passes one
/// [`RenderRoot::rebuild`] will run before deferring the rest to the next frame.
///
/// A flushed pop-result callback may itself push or pop, queueing another
/// callback — so the flush/re-diff cycle has to be allowed to iterate, but it
/// must never be allowed to spin: a pair of callbacks that push each other would
/// otherwise hang the frame. Three passes covers every shape observed in
/// practice (a result that navigates once, and that page's own result), while
/// keeping the worst case at four build-closure runs per frame — the closure is
/// cheap by construction (see [`RenderRoot::rebuild`]).
///
/// Past the cap the mark stays raised and one more frame is requested, so the
/// remaining work lands next frame instead of being lost.
const MAX_PENDING_RESULT_FLUSH_PASSES: usize = 3;

/// The allocator behind [`RenderRoot::root_identity`], handing every root a
/// value no other root shares.
///
/// Starts at `1` so `0` stays available as "no root" (a pod that has never held a
/// claim, and the at-rest published hover link — see
/// `crate::event::set_live_hover_link`).
///
/// A module-level static rather than an associated const/`static` inside the
/// generic `impl`: the latter is monomorphized per `<State, V>` pair, which would
/// hand two roots of different concrete types the same identity — exactly the
/// collision this counter exists to remove. `Relaxed` is enough because the value
/// is only ever compared for equality, never used to order anything.
static NEXT_ROOT_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// The next focus epoch after `epoch`, skipping `0`.
///
/// `0` is reserved twice over — it is a never-claimed
/// [`ChildPod`](crate::widget::ChildPod)'s stamp, and it is the epoch half of
/// the `(0, 0)` pair a dispatch driven with no root at all compares against — so
/// a root that wrapped onto it would hand every unclaimed pod in the tree a live
/// link at once.
fn advance_focus_epoch(epoch: u64) -> u64 {
    match epoch.wrapping_add(1) {
        0 => 1,
        next => next,
    }
}

/// What [`RenderRoot`]'s overlay pre-pass decided about one incoming event.
///
/// The pre-pass runs before anything else [`RenderRoot::event`] does, and has
/// exactly two answers: the event belonged to a floated surface (or was swallowed
/// by a modal light-dismiss) and the main tree must not see it, or it did not and
/// today's dispatch continues. Both arms carry an [`EventOutcome`], because even
/// the "continue" answer may already have produced one — an
/// [`OutsideTap::Notify`](crate::overlay::OutsideTap::Notify)`{ consume: false }` surface is told about the press
/// *and* lets it through, and the redraw that notification asked for must not be
/// dropped on the floor when the main dispatch's own outcome replaces it.
enum OverlayRoute {
    /// The overlay layer consumed the event; return this outcome unchanged.
    Consumed(EventOutcome),
    /// The event continues into today's dispatch; merge this outcome into
    /// whatever that produces.
    Continue(EventOutcome),
}

/// Owns the retained tree and drives the rebuild/layout/paint passes for a
/// single-root application.
///
/// Generic over the application `State` and the concrete root view type `V`
/// returned by the build closure.
pub struct RenderRoot<State: 'static, V: View<State>> {
    tree: WidgetTree,
    root_id: Option<WidgetId>,
    /// The previous view, retained to diff against on the next rebuild.
    prev_view: Option<V>,
    /// Monotonic widget-id counter, borrowed by each `BuildCtx`.
    next_id: u64,
    window_size: Size,
    /// The **claimant** of the pointer capture in flight, if any: the contact
    /// whose `Down` requested capture. Set on that `Down`, cleared only by the
    /// claimant's own `Up`/`Cancel` — another contact's release never touches
    /// it (rule (c) of [`InputEvent::PointerContact`](crate::event::InputEvent::PointerContact)'s multi-contact contract).
    /// Root-level mirror of the per-container `active` path bookkeeping, which
    /// is keyed on the same claimant (see [`crate::widget::ChildPod::set_active`]).
    capture_claimant: Option<PointerId>,
    /// Whether the live capture's captor opted into the gesture's other
    /// contacts ([`EventCtx::capture_contacts`](crate::event::EventCtx::capture_contacts)) on the `Down` it captured
    /// with. Meaningless — and kept `false` — while nothing is captured.
    /// Cleared early when a container takes the gesture over from that captor
    /// ([`EventCtx::release_captured_child`](crate::event::EventCtx::release_captured_child)).
    capture_contacts: bool,
    /// Whether the live opt-in was made by the root widget itself rather than
    /// by a pod below it — in which case a non-claimant contact is handed to the
    /// root widget directly instead of walking the active path (see
    /// [`crate::widget::ChildPod::event_child`]). Same lifetime as
    /// `capture_contacts`.
    contacts_captor_is_root: bool,
    /// Whether some widget in the tree currently holds focus. Root-level mirror of
    /// the per-container `focused` path bookkeeping (the focus analog of
    /// `capture_claimant`): set when a dispatch requested focus, cleared by a
    /// session release — a blur-on-outside-tap `Down`, an explicit focus release,
    /// a widget publishing an inactive IME surface, or the generic-unmount orphan
    /// drain in [`RenderRoot::rebuild`] (see [`release_focus_session_in`](session::release_focus_session_in)).
    focus_active: bool,
    /// Which branch the live focus/IME session belongs to: `None` for the main
    /// tree, `Some(key)` for the floated surface whose pod holds the recorded
    /// focus path. The identity `focus_active` deliberately does not carry — one
    /// bool cannot say *whose* session it is, and a surface's chain and the main
    /// tree's are not siblings any container's blur sweep can reach across.
    ///
    /// **Resolved from links that can be shown to be live.** The only
    /// authoritative view of a pod's recorded link the root ever gets is the pod
    /// itself, which it holds for exactly the length of
    /// [`RenderRoot::paint_overlays`] — so that pass writes this, from
    /// [`crate::widget::ChildPod::holds_live_focus`], and the event pass only ever
    /// *clears* it (a hit-tested claim is the main tree's by construction, and a
    /// release ends the session outright). An overlay-pass focus request cannot
    /// be attributed at the root: the bubble is a bare flag, and a field in the
    /// main tree re-claiming its own session through a floated toolbar raises
    /// exactly the same one as an editable inside the surface claiming it for the
    /// first time.
    ///
    /// The liveness half is what makes the record worth keeping. A pod's raw
    /// `focused` flag survives the session moving away from it — nothing visits
    /// an abandoned branch to clear one — so a record resolved from the flag
    /// alone latched on the first surface that ever took focus and never let go.
    /// Resolved from the stamp instead, it answers a question the code can
    /// falsify, and it names nobody the moment the session leaves every surface.
    ///
    /// **It no longer gates the tree's paint seed**, which is the other half of
    /// the same correction: seeding is per-link, against `focus_epoch`, so a
    /// branch proves its own claim rather than the root vouching for it from one
    /// frame behind. What is left here is *provenance* — whether an overlay
    /// dispatch's IME publish is the session owner's — plus the cross-surface
    /// retirement in [`RenderRoot::event`], both of which genuinely need a name
    /// rather than a per-link answer.
    focus_surface: Option<OverlayKey>,
    /// The identity of the live focus session — what a
    /// [`ChildPod`](crate::widget::ChildPod) stamps beside its recorded focus
    /// link, and the only thing that tells a link on the session the root has
    /// now from one a moved session left behind.
    ///
    /// The focus analog of `hover_epoch`, with one difference that follows from
    /// focus having no per-pass rhythm: this advances **around a dispatch**
    /// rather than at the end of one. [`RenderRoot::event`] moves it forward
    /// before it dispatches and puts it back afterwards unless the dispatch
    /// actually recorded a claim — so a claim is stamped with a value nothing
    /// older carries, and a pass that moved no focus leaves every standing link
    /// exactly as it found it. Published to the pods through
    /// [`crate::widget::set_live_focus_session`], which is where a container
    /// deciding routing, and a pod's own destructor, read it.
    ///
    /// Starts at `1`, not `0`: a freshly built pod's stamp is `0`, and `(0, 0)`
    /// is what a dispatch driven with no root at all sees, so a real root must
    /// never publish that pair.
    ///
    /// Wrapping is deliberate and harmless — the value is only ever compared for
    /// equality, never ordered — but it skips `0` on the way round (see
    /// [`advance_focus_epoch`]).
    focus_epoch: u64,
    /// Whether the last completed hover pass left some widget in the tree holding
    /// the hover link. Root-level mirror of the per-pod hover stamp (the hover
    /// analog of `focus_active`), seeded into every event/paint pass so nothing
    /// below can read as hovered while the root says nothing is.
    hover_active: bool,
    /// The live hover epoch: the identity of the most recent completed hover pass.
    ///
    /// Advanced by exactly one per hover pass — an **uncaptured**
    /// [`PointerPhase::Move`](crate::event::PointerPhase::Move) (which may record a claim), or the `Down`/`Up`/
    /// `Cancel` that ends a hover outright (which may not) — and by nothing else,
    /// so a scroll, key, IME, or housekeeping pass leaves a live hover standing.
    /// A [`crate::widget::ChildPod`]'s recorded stamp counts as hovered only while
    /// it equals this, which is what strands the previous claimant's path with no
    /// container having to clear it (see the [`crate::event`] module docs).
    ///
    /// Starts at `1`, not `0`: a freshly built pod's stamp is `0`, and starting the
    /// epoch past it means a never-claimed pod cannot match the live epoch by
    /// accident before the first hover pass ever runs.
    hover_epoch: u64,
    /// This root's process-unique identity, assigned once at construction from
    /// [`NEXT_ROOT_IDENTITY`] and never reused.
    ///
    /// It exists for exactly one comparison: the hover-orphan channel
    /// (`crate::event`'s `mark_hover_orphaned`/`take_hover_orphaned`) is a
    /// thread-local a *destructor* writes, so a second root driving passes on the
    /// same thread can otherwise see a mark that is none of its business.
    /// `hover_epoch` cannot tell them apart — every root's counter starts at `1`
    /// and advances per hover pass, so two roots hold colliding integers as a rule
    /// rather than as a fluke. Publishing and draining `(identity, epoch)` is what
    /// keeps one root's unmounting claimant from ending another's live hover.
    root_identity: u64,
    /// The cursor the last cursor pass resolved — hover's sibling channel, and
    /// the value a desktop shell reads through [`RenderRoot::cursor`].
    ///
    /// Deliberately **not** derived from `hover_active`: that mirror is
    /// identity-free (it knows *that* something is hovered, not which widget or
    /// what shape it wants), so a request travels its own pass-scoped slot
    /// ([`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor)) and is resolved here.
    ///
    /// Re-resolved on every pointer [`PointerPhase::Move`](crate::event::PointerPhase::Move), captured or not:
    /// whatever the pass requested, or [`CursorIcon::Default`] when it requested
    /// nothing. Every other pass leaves it standing — see [`RenderRoot::event`]
    /// for why a `Down`/`Up` must not reset it. There is no generation counter
    /// beside it: the shell compares the value it last applied (see
    /// [`RenderRoot::cursor`]).
    cursor: CursorIcon,
    /// The text the tree last asked the shell to put on the host clipboard, or
    /// `None` once drained — the cursor's write-only sibling, resolved from the
    /// same kind of per-pass slot ([`EventCtx::write_clipboard`](crate::event::EventCtx::write_clipboard)) by the same
    /// [`RequestPass`](crate::event::RequestPass) bracket.
    ///
    /// **One-shot, unlike [`cursor`](RenderRoot::cursor).** A cursor is a *level*
    /// (a standing shape a shell re-applies when it differs); a clipboard write
    /// is an *edge* (a thing to do once), so the accessor
    /// [`RenderRoot::take_clipboard_write`] drains it and a shell that forgets to
    /// call it merely delays the write rather than repeating it.
    ///
    /// A pass that writes replaces whatever stood here undrained — the newest
    /// copy is the one the user meant, and the shell is expected to drain after
    /// every dispatch — while a pass that writes nothing leaves it alone rather
    /// than silently discarding a write nobody has taken yet.
    pending_clipboard_write: Option<String>,
    /// Whether the tree has asked the shell to read the host clipboard back to it
    /// ([`EventCtx::request_paste`](crate::event::EventCtx::request_paste)), until drained by
    /// [`RenderRoot::take_paste_request`].
    ///
    /// The data-free twin of [`pending_clipboard_write`](RenderRoot::pending_clipboard_write),
    /// and one-shot for the same reason. Raised by any pass in which a widget
    /// asked and lowered only by the drain, so a shell that skips a drain answers
    /// late rather than losing the paste.
    pending_paste_request: bool,
    /// The IME surface the focused widget last published (via
    /// [`EventCtx::publish_ime_state`](crate::event::EventCtx::publish_ime_state)), surfaced to the shell by
    /// [`RenderRoot::ime_state`]. Persists across rebuilds/events until refreshed
    /// by a new publish or dropped by a release (a blur, a focus release, an
    /// inactive publish, or a generic-unmount orphan drain — see
    /// [`release_focus_session_in`](session::release_focus_session_in)).
    ///
    /// Only ever `None` or an **active** surface: an inactive publish is a
    /// release, never a stored value (see [`RenderRoot::ime_state`]).
    ime_state: Option<ImeState>,
    /// A monotonically-increasing generation bumped on every **actual** change
    /// of `focus_active` or `ime_state` — the focus/IME session's edge signal,
    /// read by a shell through [`RenderRoot::focus_ime_generation`].
    ///
    /// The mobile frame gate turns this into an *edge* input
    /// (`FrameInputs::focus_or_ime_changed`): a shell caches the last value it
    /// saw and runs a frame when it moves. A *level* input ("something holds
    /// focus") forced a frame every vsync for as long as a field stayed focused,
    /// which made caret pacing unreachable — measured at 62–120 fps on a static
    /// screen whose only live input was focus (Xiaomi 12).
    ///
    /// Same-value writes deliberately do **not** bump it (see
    /// [`RenderRoot::set_focus_active`]/[`RenderRoot::store_ime_state`]): the
    /// paint pass republishes the focused widget's IME surface on *every* frame,
    /// so bumping on write rather than on change would re-create exactly the
    /// per-vsync forcing this edge exists to remove.
    focus_ime_gen: u64,
    /// The [`PlatformViewFrame`]s the tree published during the most recent
    /// [`RenderRoot::paint`], surfaced to the shell via
    /// [`RenderRoot::platform_view_frames`]. Unlike `ime_state` above, this is
    /// REPLACED wholesale every pass (never merged with the previous one), so
    /// a pass that publishes none yields an empty `Vec` — a culled/removed
    /// slot from the prior frame does not linger as a stale frame. Core stays
    /// dumb here: the shell's differ owns absent-means-hide/dispose semantics.
    platform_view_frames: Vec<PlatformViewFrame>,
    /// The z-shield rects the tree reported during the most recent
    /// [`RenderRoot::paint`] (via [`crate::widget::PaintCtx::report_input_shield`]),
    /// surfaced to the shell via [`RenderRoot::input_shields`].
    ///
    /// Exactly the `platform_view_frames` discipline above — REPLACED wholesale
    /// every pass, so a pass whose shields stopped painting reports none. Core
    /// stays dumb: it never associates a shield with a slot, that is the
    /// shell-side differ's job.
    input_shields: Vec<Rect>,
    /// Dirtiness accumulated since the last [`RenderRoot::take_change_flags`] —
    /// merged from each rebuild so a shell can decide, in one place, whether a
    /// frame needs layout/paint at all.
    pending: ChangeFlags,
    /// The app's active theme, stored type-erased so `frust-core` needs no
    /// `frust-theme` dependency (the concrete `Theme` is boxed by the shell —
    /// see [`RenderRoot::set_theme`]). Lent as `Option<&dyn Any>` into each
    /// [`LayoutCtx`](crate::widget::LayoutCtx)/[`PaintCtx`](crate::widget::PaintCtx); `None` until a shell sets one (a supported
    /// state — bare-core tests and pre-theme apps run without a theme).
    theme: Option<Box<dyn Any>>,
    /// The window's insets ([`WindowInsets`]), delivered by the shell via
    /// [`RenderRoot::set_insets`] and threaded into every subsequent
    /// layout/paint pass (recovered by widgets through
    /// [`crate::widget::LayoutCtx::window_insets`]/
    /// [`crate::widget::PaintCtx::window_insets`]). Unlike the theme this is a
    /// concrete core-owned type (only `f64` scalars), stored by value — no
    /// `Box<dyn Any>` erasure needed. Defaults to the zero inset until a shell
    /// pushes one (a supported state — bare-core tests and pre-insets apps).
    insets: WindowInsets,
    /// The shell's running count of frames the render thread has actually
    /// presented, threaded into every subsequent paint pass and recovered by
    /// widgets through [`crate::widget::PaintCtx::presented_frames`]. A plain
    /// `u64` core stores by value (like the insets). `None` until a shell pushes
    /// one via [`RenderRoot::set_presented_frames`] — a supported state
    /// (bare-core tests and pre-wiring shells run without it), so widgets can
    /// fall back to a paint-cadence measure. Unlike the theme/insets this is a
    /// pure observation: [`RenderRoot::set_presented_frames`] deliberately marks
    /// NO [`ChangeFlags`] and bumps NO semantics generation (see its doc), so a
    /// ticking presented count never forces a relayout or feeds the mobile frame
    /// gate.
    presented_frames: Option<u64>,
    /// Whether the shell created a translucent (alpha-channel, "Mode B") GPU
    /// surface, threaded into every subsequent paint pass and recovered by
    /// widgets through [`crate::widget::PaintCtx::is_translucent`]. A plain
    /// `bool` core stores by value (like the insets); `false` (opaque, "Mode A")
    /// until a shell pushes one via [`RenderRoot::set_surface_translucent`] — the
    /// supported default for every desktop app and bare-core test. The
    /// platform-view hole-punch is the sole reader: a slot clears its rect only
    /// on a translucent surface (see `frust-widgets`' `PlatformViewWidget`).
    surface_translucent: bool,
    /// The persistent, never-reused per-pod semantics base-id allocator's next
    /// value. Seeded at `2` (ids `0`/`1` reserved: `0` keeps
    /// `NonZeroU64` valid, `1` is the [`ROOT_NODE_ID`](crate::semantics::ROOT_NODE_ID) window node), advanced as
    /// [`ChildPod`](crate::widget::ChildPod)s are assigned bases on their first
    /// semantics visit, and carried across passes so a pod that first appears on a
    /// later frame never collides with an already-assigned one. A `Cell` because
    /// [`RenderRoot::semantics`] runs behind `&self`.
    semantics_alloc: Cell<u64>,
    /// The root widget's stable semantics base id (the root pod is arena-backed,
    /// not a [`ChildPod`](crate::widget::ChildPod), so it caches its base here
    /// rather than in a pod). Lazily assigned on the first semantics pass.
    root_semantics_id: Cell<Option<NonZeroU64>>,
    /// A monotonically-increasing generation bumped whenever a rebuild or theme
    /// swap could have changed the semantics tree, so a shell can cheaply skip
    /// re-pulling + re-pushing an unchanged accessibility tree (the semantics
    /// dirty gate — see [`RenderRoot::semantics_if_changed`]). v1 recompute is
    /// acceptable; this is the seam a shell gates on.
    semantics_gen: u64,
    /// Set by [`RenderRoot::rebuild`] when the deferred-callback flush owes the
    /// shell a frame, and folded into the next [`RenderRoot::paint`]'s
    /// [`PaintOutcome::needs_frame`](crate::widget::PaintOutcome::needs_frame) (then cleared). Two raisers, both in the
    /// flush loop: hitting [`MAX_PENDING_RESULT_FLUSH_PASSES`] with work still
    /// owed, and a dispatched [`InputEvent::Housekeeping`](crate::event::InputEvent::Housekeeping) whose
    /// [`EventOutcome::needs_redraw`] came back set.
    ///
    /// The frame-request half of the deferral: `pending |= PAINT` already tells
    /// the mobile frame gate to run its next tick, but the desktop loop is
    /// dirty-driven (`ControlFlow::Wait`) and schedules off `needs_frame`, so the
    /// deferral has to surface there too — otherwise the remaining flush would
    /// wait for whatever input happens to arrive next, which is the exact
    /// failure this whole mechanism exists to remove.
    deferred_frame: bool,
    /// Whether the last main-tree paint asked for a frame or a layout — the
    /// root widget's own copy of `ChildPod`'s trailing-paint bit. The root
    /// widget sits in no pod, so nothing else remembers that the paint an
    /// animation lands on (the first that asks for nothing) still owes one
    /// layout pass; [`RenderRoot::paint`] folds `LAYOUT` for it once.
    painted_animating: bool,
    /// The routing half of the overlay entries the **last** [`RenderRoot::paint`]
    /// registered, in paint order (`Floating` band then `Tooltip`, registration
    /// order within each) — what [`RenderRoot::event`]'s overlay pre-pass
    /// hit-tests before the main tree ever sees a pointer.
    ///
    /// Replaced wholesale every paint, exactly like `platform_view_frames`: an
    /// owner keeps a surface routable by registering it again each frame, so a
    /// surface whose owner stopped registering (or was unmounted) stops taking
    /// input after the next paint with nothing to unregister.
    ///
    /// Carries **no pod handle** by construction (see
    /// [`OverlayHit`](crate::overlay::OverlayHit)): the owner owns the pod, and a
    /// root holding a clone of it between passes would both outlive the owner and
    /// invite a borrow held across a pass boundary.
    ///
    /// One frame of lag is inherent and intended: input is routed against where
    /// the surfaces were painted, which is the only place the user could have
    /// seen them.
    overlay_hits: Vec<OverlayHit>,
    /// The selection-toolbar request the focused field published during the most
    /// recent [`RenderRoot::paint`], surfaced to the shell through
    /// [`RenderRoot::selection_toolbar`] for the platform edit-menu route.
    ///
    /// Resolved per paint pass: a pass in which nothing published clears it, which
    /// is what puts the menu away when a selection collapses. A session release
    /// clears it too (see [`RenderRoot::release_focus_session`]), so the menu can
    /// never outlive the focus the selection belonged to — the event pass's blur
    /// lands a whole frame before the paint that would otherwise notice.
    selection_toolbar: Option<SelectionToolbarRequest>,
    /// A monotonically-increasing generation bumped on every **actual** change of
    /// `selection_toolbar` — the same change-guarded edge signal
    /// `focus_ime_gen` is, and for the same reason: the publishing field
    /// re-publishes an unchanged request every single frame its selection stands,
    /// so bumping on write rather than on change would ask the shell to re-present
    /// the platform menu on every vsync.
    ///
    /// Kept beside the value rather than inside it so the counter survives a
    /// clear: a shell diffs the generation to notice the menu went *away* just as
    /// much as to notice it appeared.
    selection_toolbar_gen: u64,
    /// The global layout epoch threaded into every layout pass
    /// ([`LayoutCtx::layout_epoch`](crate::widget::LayoutCtx::layout_epoch)). Bumped for each global layout input that
    /// does not reach a widget through its constraints — a theme swap
    /// ([`RenderRoot::set_theme`], which is also the path a shell's text-context
    /// font drain takes), an actual inset change ([`RenderRoot::set_insets`]), a
    /// window-size change seen by [`RenderRoot::layout`] — and when a floated
    /// surface's pod is found dirty (see `overlay_pods`). A pod that last laid
    /// out under another epoch lays out again, so a bump invalidates every
    /// cached layout at once. Starts at `1`, past a fresh pod's `0`.
    layout_epoch: u64,
    /// The paint epoch threaded into every paint pass ([`PaintCtx::subtree_clean`](crate::widget::PaintCtx::subtree_clean)):
    /// bumped with every layout-epoch bump, by a surface-translucency flip, and
    /// at the start of a paint that follows a hover-link or focus-session
    /// change (see `painted_links`), so a pod painted under another epoch is
    /// never reported clean.
    paint_epoch: u64,
    /// The serial of the most recent [`RenderRoot::paint`] (`0` before the
    /// first): each paint mints the next one, threads it to every pod
    /// ([`PaintCtx::paint_serial`](crate::widget::PaintCtx::paint_serial)) and reports it on
    /// [`PaintOutcome::paint_serial`](crate::widget::PaintOutcome::paint_serial).
    paint_serial: u64,
    /// The hover link as the last completed hover pass left it: `None` while
    /// nothing holds hover, otherwise the claimant pod's
    /// [`ChildPod::inspect_id`](crate::widget::ChildPod::inspect_id) (`0` for a
    /// claim the root widget made itself). Unlike `hover_epoch`, which advances
    /// on every hover pass, this moves only when hover begins, moves to another
    /// claimant, or ends.
    hover_link: Option<u64>,
    /// `(hover_link, focus_epoch)` as the last paint saw them. Both are read at
    /// paint time ([`PaintCtx::is_hovered`](crate::widget::PaintCtx::is_hovered), [`PaintCtx::has_focus`](crate::widget::PaintCtx::has_focus)) from links
    /// the root moves without visiting the pod that loses one, so a paint that
    /// finds either changed bumps `paint_epoch`: no retained page recorded
    /// under the old link may be drawn under the new one.
    painted_links: (Option<u64>, u64),
    /// Whether retained layers are enabled ([`RenderRoot::set_retained_layers`]).
    retained_layers: bool,
    /// The renderer's resident-key ledger as of the previous frame
    /// ([`RenderRoot::set_retained_ledger`]); empty until a shell sets one.
    retained_ledger: crate::retained::RetainedLedger,
    /// Whether layout passes may skip clean pods ([`RenderRoot::set_layout_skip`]);
    /// `true` by default. `false` forces every pod to lay out every pass — the
    /// kill switch a shell wires to an environment variable (this crate reads
    /// none itself).
    layout_skip: bool,
    /// The layout audit's state ([`RenderRoot::set_layout_audit`]); off by default.
    layout_audit: LayoutAudit,
    /// Weak handles to the pods of the floated surfaces the **last** paint
    /// drew, refreshed every paint beside `overlay_hits`.
    ///
    /// A floated pod is laid out by its *owner* (inside the owner's own
    /// `layout`) but painted and fed paint-time requests by this root, outside
    /// the owner's pod chain — so a paint-time frame request or a substituted
    /// event dispatch can dirty the floated pod while every pod above its owner
    /// stays clean, and the owner's layout (the only route to the floated pod)
    /// would be skipped. [`RenderRoot::layout`] therefore checks these first and
    /// bumps the epoch when any is dirty: conservative (the whole tree lays out
    /// while a surface animates) but correct.
    ///
    /// Weak, never strong, so the root still co-owns no pod (see
    /// `overlay_hits`): a surface whose owner dropped it simply fails to
    /// upgrade. The check borrows each pod only for the length of one flag read,
    /// at the start of a layout pass, when no pass holds a borrow.
    overlay_pods: Vec<Weak<RefCell<ChildPod>>>,
    _state: core::marker::PhantomData<fn(&mut State)>,
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Create an empty render root with no widget yet built.
    pub fn new() -> Self {
        Self {
            tree: WidgetTree::new(),
            root_id: None,
            prev_view: None,
            next_id: 0,
            window_size: Size::ZERO,
            capture_claimant: None,
            capture_contacts: false,
            contacts_captor_is_root: false,
            focus_active: false,
            focus_surface: None,
            // Past a fresh pod's `0` stamp, and past the `(0, 0)` a rootless
            // dispatch sees — see the field doc.
            focus_epoch: 1,
            hover_active: false,
            // Past a fresh pod's `0` stamp — see the field doc.
            hover_epoch: 1,
            root_identity: NEXT_ROOT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            cursor: CursorIcon::Default,
            pending_clipboard_write: None,
            pending_paste_request: false,
            ime_state: None,
            focus_ime_gen: 0,
            platform_view_frames: Vec::new(),
            input_shields: Vec::new(),
            pending: ChangeFlags::NONE,
            theme: None,
            insets: WindowInsets::default(),
            presented_frames: None,
            surface_translucent: false,
            // Ids 0 and 1 are reserved (see the field doc); pods start at 2.
            semantics_alloc: Cell::new(2),
            root_semantics_id: Cell::new(None),
            semantics_gen: 0,
            deferred_frame: false,
            painted_animating: false,
            overlay_hits: Vec::new(),
            selection_toolbar: None,
            selection_toolbar_gen: 0,
            layout_epoch: 1,
            paint_epoch: 1,
            paint_serial: 0,
            hover_link: None,
            // The links a fresh root starts with (no hover, focus epoch `1`).
            painted_links: (None, 1),
            retained_layers: true,
            retained_ledger: crate::retained::RetainedLedger::empty(),
            layout_skip: true,
            layout_audit: LayoutAudit::default(),
            overlay_pods: Vec::new(),
            _state: core::marker::PhantomData,
        }
    }
}

impl<State: 'static, V: View<State>> Default for RenderRoot<State, V> {
    fn default() -> Self {
        Self::new()
    }
}
