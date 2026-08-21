//! The virtualized `ListView`:
//! `ListView::builder(item_count, item_extent, |index| -> AnyView)` with a
//! uniform, required `item_extent` — or, on a keyed list,
//! `ListView::builder_keyed(..).estimated_item_extent(px)` for rows that size
//! themselves (see *Variable extents*, below). Its place in the widget set and
//! why virtualization exists at all belong to `docs/WIDGETS_ARCHITECTURE.md`
//! (*Virtualized ListView*); this header holds the contracts behind that summary.
//!
//! # Windowed materialization at rebuild time
//!
//! Children materialize only at [`View::rebuild`] time — never at layout, the
//! spoke doc's windowing invariant — and `rebuild` receives the retained
//! `&mut Self::Element`, so it reads the widget's *own* scroll offset and cached
//! viewport, recomputes the window
//! `[floor(offset/extent) - BUFFER, ceil((offset+viewport.h)/extent) + BUFFER]`
//! (clamped to `[0, item_count]`) and reconciles the live children to it,
//! **keyed by item index**: an index staying in the window keeps its live
//! [`ChildPod`] — and all of that child's retained state — *relocated* into the
//! new order and rebuilt in place against its own previous view; indices leaving
//! are torn down, indices entering are built fresh. The builder being a pure
//! function of the index, a survivor's previous view is `prev.builder(i)` and its
//! current one `self.builder(i)`, so no per-window view cache is retained.
//!
//! # Row identity: positional by default, keyed on request
//!
//! [`ListView::builder`] reconciles rows by raw item index,
//! [`ListView::builder_keyed`] by a caller-supplied `key_of(index) -> ChildKey`
//! (the same [`ChildKey`] [`keyed`](crate::keyed) uses for `Flex`); which data
//! mutations each model is correct for, and how a positional list silently
//! reattaches state on a mid-list insert/remove/reorder, is the spoke doc's *Two
//! row-identity models*. What a retained [`ChildPod`] carries through either is
//! that row's *entire* state — hosted `Component` state, `StateLayer` flags, a
//! `ListItem`'s toggle/press state — which is what makes that misattachment
//! silent rather than a panic. Keying is the fix; two hard parts:
//!
//! * **Slots vs. identity.** Window slots stay the contiguous ascending index run
//!   the uniform-extent fast path (`window_covers`, the `item_count *
//!   item_extent` scroll extent) relies on; identity lives *beside* them, in a
//!   retained `key -> the index that row occupied last frame` map rebuild reads
//!   off the element like `offset` and `viewport` (the view itself remembers
//!   nothing, being rebuilt from scratch every frame). Naming a survivor's last
//!   index is what redirects the reconstruction above through the map, as
//!   `prev.builder(prev_index)` against `self.builder(index)`; keys leaving the
//!   window or the data are torn down, keys entering built fresh.
//! * **Duplicate keys.** Ambiguous, so a duplicate trips a `debug_assert!`,
//!   mirroring [`crate::authoring::rebuild_children`]'s keyed reconciler — but
//!   with **no positional fallback** in release: first slot wins, the map keeps
//!   the last index, and which row keeps its state is arbitrary.
//!
//! Keying costs one `key_of` per *materialized* slot per frame plus that map,
//! never anything `item_count`-sized, and leaves the positional path untouched.
//! Either path reports `ChangeFlags::LAYOUT` on any frame that built, tore down,
//! relocated, or re-ranged pods — a freshly built or relocated pod has never been
//! laid out where it now sits, and a frame skipping layout would paint it unsized
//! or at a stale origin.
//!
//! # Prepend/removal scroll anchoring (keyed lists only)
//!
//! A keyed list also corrects the scroll offset so a mutation **above** the
//! viewport — a "load older" prepend, a removal above the screen — doesn't
//! visually jump the content the user is looking at. It runs inside
//! [`View::rebuild`], **before** this frame's window is recomputed, so layout and
//! paint agree on one corrected offset; never during paint.
//!
//! **Anchor selection.** The anchor is the topmost surviving keyed row of the
//! previous window: walking [`ListViewWidget::keys`] ascending, the first key
//! that still identifies a row in the new data wins. That is checked two ways, at
//! most two extra `key_of` calls per candidate — unchanged position
//! (`self.key_of(i_prev) == anchor_key`) or shifted by this frame's net
//! `item_count` delta (`self.key_of(i_prev + delta) == anchor_key`), the latter
//! correct precisely because nothing between the mutation and the anchor changed,
//! so every surviving row at or above the old window shifts by the same net
//! count. If neither matches for any key in the previous window, no correction
//! runs at all: a full replace is reset semantics, not an anchoring case. The
//! search is bounded by the window size, never `item_count`.
//!
//! **Correction.** A confirmed shift of `d` items applies
//! `offset += d as f64 * item_extent`, clamped to `[0, max_offset]` and reported
//! as `ChangeFlags::LAYOUT` — closed form, since `d` is the item-count delta
//! itself merely confirmed against the anchor key, so nothing searches for
//! *where* the anchor landed.
//!
//! **Anchor always, even from `offset == 0`**: pinning `offset` at zero on a
//! prepend would snap that same old top row back under the user, so this module
//! shifts instead, revealing the newly-prepended rows. A prepend answering a
//! fired [`ListView::on_near_start`] also leaves that edge's armed flag as the
//! fire left it (disarmed) rather than letting the unconditional item-count
//! rearm force it back armed, which would re-trigger "load older" on the very
//! next scroll event.
//!
//! **Positional lists are never anchored** — position-only identity cannot tell a
//! prepend from a full mutation, so the correction would be a guess.
//!
//! # Variable extents (keyed lists only)
//!
//! [`ListView::estimated_item_extent`] switches a **keyed** list into
//! variable-extent mode: a row not yet laid out is assumed `estimate` tall, one
//! already laid out contributes its own measured height. Without that call the
//! list stays on the closed-form uniform path, kept literal rather than folded
//! into this math, as the regression guard.
//!
//! **Keyed-only, enforced by a `debug_assert`** that is **inert** in release (the
//! list stays uniform), the duplicate-key tripwire's shape: a measured extent is
//! cached under the row's *identity*, so under positional identity it would
//! reattach to whatever content later occupies the index. (Type-state would make
//! the misuse a compile error, at the cost of two generic `ListView` families.)
//!
//! **The extent model.** [`ListViewWidget`] retains `key -> (last index, measured
//! height)` for every row it has ever laid out, plus their running sum, so
//!
//! ```text
//! total = Σ measured + estimate × (item_count − measured count)
//! ```
//!
//! is O(1) to read — and `max_offset`, the offset clamp, the unbounded-height
//! layout size and the semantics scroll range all read it, converging on the true
//! content height as rows are visited.
//!
//! **Offset → index in O(window + step), never O(N).** A full prefix sum from
//! item 0 would be O(N) per frame, so the widget retains one *prefix anchor*: the
//! index the materialized window starts at, plus that item's content-space `y`.
//! Each frame's window walks from there — a handful of items for a scroll or a
//! fling step — accumulating measured-or-estimated extents until it reaches the
//! offset, then out to cover the viewport ± [`BUFFER`]; each row is then placed
//! at its own walked content `y` (`slot_y`) minus the offset. Reaching item 0
//! re-pins `y = 0` exactly, erasing accumulated drift, and a jump farther than
//! [`MAX_PREFIX_STEP`] items counts as a data reset rather than a scroll: its
//! bulk resolves in closed form against the estimate, only the remainder walks.
//!
//! The builder still never runs outside rebuild, so a measurement that leaves the
//! window no longer covering the viewport costs one convergence frame (*Viewport
//! staleness*, below), never a layout-time build.
//!
//! **Cache hygiene.** The measured cache is bounded by the keys a session has
//! actually visited (one `f64` + index per visited row; an LRU cap is a named
//! deferral). Three rules trim it, all window-bounded or shrink-only:
//!
//! * a pod no slot claimed is probed with anchoring's same two hypotheses; if
//!   neither still names its key the row left the *data*, not just the window,
//!   and its measurement is dropped — two `key_of` calls per departing row. This
//!   runs on every reconciled frame that isn't a genuine full replace (third
//!   rule), *including* one where the anchor-shift probe missed: that miss proves
//!   only that no uniform shift explained the whole previous window at once, never
//!   that *this* row's own two hypotheses fail, and skipping it there leaks
//!   permanently, a removed row having no later shrink or revisit to reclaim it,
//! * a frame whose `item_count` shrank drops every entry whose recorded index is
//!   past the new end — the only entries the new keying provably cannot produce,
//!   scanned only on a shrink frame,
//! * a wholesale replace clears the cache outright, but is decided against the
//!   reconciled window's own *exact* key matches (`ListView::reconcile_keyed`'s
//!   per-slot lookup), never the anchor-shift probe: that probe tests only two
//!   candidate index shifts and can miss a same-frame mutation touching both
//!   sides of the anchor while the on-screen rows are unchanged. Only a frame
//!   where none of the previous window's keys matched a slot is a genuine
//!   replace.
//!
//! Two acknowledged gaps: a row removed while *outside* the materialized window
//! keeps its entry (finding it would mean re-keying all `item_count` items, the
//! O(N) this design exists to avoid); and a same-frame mutation on both sides of
//! the anchor can still miss the anchor-shift probe itself, leaving the *scroll
//! position* uncorrected for that one frame (a visible jump), any pending
//! measured-anchor correction being discarded rather than committed into geometry
//! it can no longer explain. Both cost accuracy in an already-estimated total,
//! never a misattached measurement or a leaked entry.
//!
//! # Measured anchor correction (variable extents only)
//!
//! A row measuring taller or shorter than it was *assumed* to be moves every row
//! below it, including the one the viewport's top edge sits inside — the
//! **anchor**. So layout accumulates, over the rows lying wholly above that edge
//! *in the geometry this frame's window was planned against*, the signed
//! `measured − assumed` delta into one [`ListViewWidget::pending_correction`]:
//! what the offset owes to leave the anchor row exactly where it is. Rows at or
//! below the anchor contribute nothing — a row growing pushes content below it
//! down, the truth, not a jump.
//!
//! **Recorded at layout, committed at the next rebuild.** A *scroll* correction
//! is neither layout's nor paint's to make — layout's only offset write stays the
//! range clamp, paint's the fling pump — so paint asks for one continuation frame
//! while a correction is pending and the next [`View::rebuild`] commits it
//! *before* planning the window, folding `ChangeFlags::LAYOUT` into its report.
//! The pending amount is nevertheless **honored visually the instant it is
//! measured**: every reader that *places* content — the prefix walk, the window,
//! each row's origin, the edge triggers, the semantics scroll position — reads
//! [`ListViewWidget::placement_offset`] (`offset + pending`, clamped) instead of
//! the raw offset, so the anchor row never moves and committing is pure
//! bookkeeping. The raw `offset` — the fling pump's, the drag's, the wheel's and
//! the clamp arithmetic's — moves only from input or a rebuild-time correction.
//!
//! **Fling interplay: accumulate, then apply at settle.** While a fling is live
//! the pump advances `offset` at paint, and committing per-frame would fight it
//! two ways: a correction *opposing* the fling exceeds a decayed fling step near
//! the end of the animation and walks the offset backwards, and one *along* it
//! can push the offset onto a bound, where [`ListViewWidget::tick`]'s at-bound
//! check kills the fling early. So a correction taken during a fling only
//! accumulates; the first rebuild after the fling stops (velocity below
//! [`FLING_STOP`], a bound reached, or a `Down` taking the gesture over — all
//! three clear `fling`) commits the whole sum, invisibly, since placement honored
//! it every frame anyway. The reversal is measured, not assumed: deleting the
//! withhold makes this module's upward-fling test walk the offset backwards
//! mid-flight (`7980 → 7993`, ~13px against the gesture). **Accepted artifact:**
//! until a long fling over never-measured rows settles, the *committed* offset
//! lags the painted placement by exactly the accumulated sum, so a reader of
//! [`ListViewWidget::offset`] alone sees a stale scroll position.
//!
//! **Clamping and edge triggers.** A correction goes through the same
//! `[0, max_offset]` clamp as every other offset write, against a `max_offset`
//! itself moving as measurements revise the content extent; one clamped at an
//! edge is truncated, not kept owing, so the top/bottom of the list wins over
//! anchor fidelity. `near_start`/`near_end` evaluate on events and on the fling
//! pump, never in rebuild, so a commit can never itself fire one; and because
//! they read the *placement*, which a commit leaves unchanged, a correction
//! cannot rearm or re-fire an edge that has not genuinely moved.
//!
//! **What stays estimated.** Only materialized rows are ever measured, so a
//! prepend landing entirely *above* the window is anchored by the estimate alone
//! — the closed-form shift above — refining only if the user scrolls back over
//! those rows; a prepend landing inside the window takes both steps in
//! consecutive frames, the estimate shift then the measured refinement.
//!
//! # Viewport staleness
//!
//! [`frust_core::BuildCtx`] carries no viewport size, so the widget caches
//! `viewport: Size` from the previous layout pass (the `ScrollWidget` precedent)
//! and rebuild reads it off the element. One frame of staleness on a constraint
//! change is accepted, and the very first `build` materializes a conservative
//! window from a zero viewport; to converge, [`Widget::paint`] requests one more
//! frame whenever the materialized window does not yet *cover* the now-known
//! viewport, so a stationary list never idles under-materialized. Coverage, not
//! equality: an over-wide window asks for no extra frame, the next rebuild trims.
//!
//! # Scroll machinery (reused, not reinvented)
//!
//! The widget owns its own vertical drag capture / wheel / fling, reusing
//! `frust-core::input`'s constants + fling math and the spring-during-paint pump
//! exactly like [`crate::ScrollView`] (see `scroll.rs`). During a scroll drag it
//! captures the pointer and, on takeover, cancels any armed child (a `ListItem`'s
//! press) via the structural-change contract — the accepted, Flutter-like
//! tradeoff. An offset change requests a redraw; the fling advances the offset at
//! paint and requests a continuation frame, so the next rebuild→layout→paint
//! re-windows as the fling carries the list.
//!
//! [`ListViewWidget::offset`] — the *windowing* offset deciding which item
//! indices materialize — is clamped to `[0, scroll extent - viewport.height]`
//! (*The extent model*, above, for that extent on each path) and never leaves
//! that range; see *Overscroll and pull-to-refresh* for the bounded out-of-range
//! *visual* displacement layered on top of it.
//!
//! # Overscroll and pull-to-refresh
//!
//! [`ListView::on_refresh_release`] and a drag's rubber-band overscroll feel are
//! [`crate::ScrollView`]'s, sharing `scroll.rs`'s `pub(crate)`
//! resistance/trigger/settle items rather than a hand-copied second set so the
//! two can never drift — the spoke doc's *Refresh/overscroll parity with
//! ScrollView*. Only the settle animation is reimplemented, against a data shape
//! `ScrollWidget` has no windowing concept to keep separate from.
//!
//! **The feel itself comes from a [`ScrollPhysics`]** ([`crate::physics`]),
//! installed as [`crate::physics::default_physics`]'s platform-adaptive choice
//! — the same seam, the same default, the same per-move drag convention, and
//! the same parity rule as `ScrollView` (see its *Physics seam*): the drag
//! mapping and boundary rejection are asked of the physics, while the legacy
//! fling/settle path and the hard-clamped wheel path stay here. What is local
//! to this widget is only how the answer is *stored* — split across a clamped
//! windowing offset and a paint-only displacement, below.
//!
//! **Windowing offset vs. painted offset.** [`ListViewWidget::offset`] *is* the
//! item-index math — where a `ScrollWidget::offset` can carry an out-of-range
//! value directly, nothing there reading it as an index — so it must stay in
//! `[0, max_offset]` at all times, and a drag past an edge splits the two:
//! `offset` stays clamped (window planning, the prefix walk and the edge triggers
//! all read [`ListViewWidget::placement_offset`], built on it),
//! [`ListViewWidget::overscroll`] carries the signed, resisted past-edge
//! displacement alone, and only [`ListViewWidget::painted_offset`]
//! (`placement_offset() + overscroll`) — read solely by
//! [`ListViewWidget::sync_child_origins`] and [`Widget::semantics`]'s scroll
//! position — ever sees the out-of-range number. The content edge visually
//! displaces; no row is ever materialized outside `[0, item_count)`.
//!
//! **How the pull is visualized is a separate axis**, shared verbatim with
//! `ScrollView` (see its *Overscroll visuals*): under the default
//! [`OverscrollEffect::Translate`] the displacement is what
//! [`ListViewWidget::painted_offset`] layers in, above; under
//! [`OverscrollEffect::Stretch`] the rows stay where the windowing offset puts
//! them and [`Widget::paint`] scales them about the held edge from
//! [`ListViewWidget::edge_pull`] instead — paint-only either way.
//!
//! **Resistance uses the *current*, converging `max_offset`.** Every drag `Move`
//! re-splits the drag position against a freshly-read
//! [`ListViewWidget::max_offset`], not a value cached at takeover, so a
//! bottom-edge overscroll in variable-extent mode tracks the content extent as
//! in-flight measurements revise it — content that grows under the finger
//! absorbs the displacement it had already produced instead of the surface
//! jumping by it.
//!
//! **`on_refresh_release` only ever arms at the top edge**, mirroring
//! `ScrollView`: it fires on `Up` when `overscroll < -REFRESH_TRIGGER_PX`, a
//! condition only the *negative* (past-top) direction can satisfy — a bottom
//! overscroll releases into a settle like an under-threshold top one, but can
//! never fire it. [`ListView::on_near_start`]/[`ListView::on_near_end`] read
//! `placement_offset()`, which overscroll never touches, so one gesture can cross
//! the near-start threshold on its way down (firing "load older") and *then* the
//! refresh trigger before release — two independent signals, not a conflict.
//!
//! **Fling and wheel stay hard-clamped**, exactly like `ScrollView`: on the
//! shipped feel only a drag ever *sets* a nonzero `overscroll` — wheel forces it
//! back to `0.0` outright, and a fling can never enter it, since
//! [`ListViewWidget::tick`]'s at-bound
//! check stops a fling the instant `offset` reaches `0`/`max_offset`. (The
//! generic ballistic driver is the one other writer, and only for a physics
//! whose simulation is *allowed* past an edge — the bouncing default's spring
//! is exactly that, `RubberBand` builds none.) A new
//! `Down` does *not* reset it, only cancelling any in-progress settle
//! (`ListViewWidget::settling = false`), so a regrab mid-bounce continues from
//! wherever the surface sits rather than snapping first — mirroring
//! `ScrollWidget::event_at`'s `Down` arm, which leaves its own `offset` untouched
//! for the same reason.
//!
//! # Nested scrolling: innermost wins
//!
//! This widget is both halves of `scroll.rs`'s innermost-wins arbitration (see
//! its *Nested scrolling*), on the same shared seam rather than a second copy:
//! it reports itself into its host's ambient claim cell
//! ([`crate::scroll::ambient_scroll_claim`]) as a forwarded `Down` reaches it,
//! and it pushes its own cell ([`crate::scroll::with_scroll_claim`]) around the
//! `Down` it routes to its own rows — in that order, so a row's own nested
//! scrollable pairs with *this* list and not with whatever encloses it. At the
//! takeover site a registered inner that can consume the drag's direction makes
//! this list defer instead of cancelling its rows. A list with no nested
//! scrollable in the window behaves exactly as it always has.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FLING_STOP,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, ScrollDelta, SemanticsCtx, TOUCH_SLOP, VelocityTracker, View, WHEEL_LINE_PX,
    Widget, fling_decay, fling_displacement,
};
use kurbo::{Point, Size};

use crate::ChildKey;
use crate::authoring::{ErasedCallback, presses};
use crate::physics::effect::OverscrollEffect;
use crate::physics::{
    MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR, ScrollMetrics, ScrollPhysics, Simulation,
    default_overscroll_effect, default_physics,
};
use crate::scroll::{
    BallisticState, InnerScrollState, METRICS_FALLBACK_DPR, SETTLE_DECAY, SETTLE_STOP_PX,
    ambient_scroll_claim, crossed_refresh_trigger, inner_claim_state, stretch_about_edge,
    with_scroll_claim,
};

/// Extra items materialized above and below the visible window, so a small
/// scroll (or a fling's per-frame advance) reveals already-built rows instead of
/// a blank edge before the next rebuild re-windows.
const BUFFER: isize = 2;

/// The debug-only tripwire message for two materialized slots claiming one
/// identity, shared by the keyed build and rebuild paths. Mirrors
/// [`crate::authoring::rebuild_children`]'s duplicate-key `debug_assert!`; unlike
/// that one there is no positional fallback to take, so release builds carry on
/// with first-claim-wins (see [`ListView::builder_keyed`]).
const DUPLICATE_KEY_MSG: &str = "ListView::builder_keyed produced a duplicate key inside one \
     window: row identity is ambiguous (release: the first slot claiming a key keeps the live \
     row, later duplicates build fresh)";

/// The debug-only tripwire message for [`ListView::estimated_item_extent`] on a
/// positional list. Variable extents cache a measurement under the row's stable
/// identity, which a positional list does not have; release builds ignore the
/// estimate and stay on the uniform path rather than panicking live (see the
/// [module docs](self)' *Variable extents* section).
const ESTIMATE_NEEDS_KEYS_MSG: &str = "ListView::estimated_item_extent requires \
     ListView::builder_keyed: a measured extent is cached under the row's stable key, which a \
     positional list has none of (release: the estimate is ignored and the uniform extent path \
     runs)";

/// The longest prefix walk a single frame will step item-by-item before
/// resolving the remaining distance in closed form against the estimate. A
/// scroll, a fling step, or a window shift moves the offset by far less than
/// this; only a data reset or a programmatic jump reaches it, and the walk's
/// result re-pins the anchor, so the very next frame is short again.
const MAX_PREFIX_STEP: usize = 512;

/// A view-held near-start "load older" callback (erased to [`ErasedCallback`] on
/// build).
type OnNearStart<State> = Rc<dyn Fn(&mut State)>;

/// A view-held near-end "load newer" callback (erased to [`ErasedCallback`] on
/// build). Mirrors [`OnNearStart`].
type OnNearEnd<State> = Rc<dyn Fn(&mut State)>;

/// A view-held pull-to-refresh release callback (erased to [`ErasedCallback`]
/// on build). Mirrors [`crate::ScrollView`]'s type of the same shape exactly —
/// see the [module docs](self)' *Overscroll and pull-to-refresh* section.
type OnRefresh<State> = Rc<dyn Fn(&mut State)>;

/// A view-held stable-key function (`item index -> row identity`), installed by
/// [`ListView::builder_keyed`] and absent for the positional
/// [`ListView::builder`]. Retained (an [`Rc`]) beside the builder, and — like the
/// builder — a pure function of the index.
type KeyOf = Rc<dyn Fn(usize) -> ChildKey>;

/// One row's cached measurement in variable-extent mode, held under the row's
/// stable key. See the [module docs](self)' *Variable extents* section.
#[derive(Clone, Copy, Debug)]
struct Measured {
    /// The item index this row occupied the last time it was measured — the
    /// only handle the cache has on "the current keying can no longer produce
    /// this entry" when `item_count` shrinks.
    index: usize,
    /// The row's laid-out height (logical px).
    extent: f64,
}

/// The window one frame should materialize: the `[start, end)` slot range plus
/// the content-space `y` of `start` (the prefix walk's result in variable-extent
/// mode; `start * item_extent` on the uniform path).
#[derive(Clone, Copy, Debug)]
struct WindowPlan {
    start: usize,
    end: usize,
    y_start: f64,
}

impl WindowPlan {
    /// The empty window (an empty list, or a degenerate extent).
    const EMPTY: Self = Self {
        start: 0,
        end: 0,
        y_start: 0.0,
    };
}

/// A declarative, virtualized vertical list. See the [module docs](self).
///
/// `builder` is a pure function of the item index; it is retained (an [`Rc`]) so
/// the previous frame's view for a surviving index can be reconstructed during
/// reconciliation. All rows share the uniform, required `item_extent`.
pub struct ListView<State: 'static> {
    item_count: usize,
    item_extent: f64,
    builder: Rc<dyn Fn(usize) -> AnyView<State>>,
    /// The stable-key function when this is a [`ListView::builder_keyed`] list;
    /// `None` selects the positional (index-identity) reconciliation path. See
    /// the [module docs](self)' *Row identity* section.
    key_of: Option<KeyOf>,
    /// The assumed extent of an unmeasured row, installed by
    /// [`ListView::estimated_item_extent`]; `None` (the default) keeps the
    /// closed-form uniform path. Only honored on a keyed list — see the
    /// [module docs](self)' *Variable extents* section.
    estimated_item_extent: Option<f64>,
    /// Fired (edge-triggered) when the scrolled window comes within
    /// `near_start_threshold` of content start — the "load older" edge. See
    /// [`ListView::on_near_start`].
    on_near_start: Option<OnNearStart<State>>,
    /// Distance from content start (px) at which `on_near_start` fires.
    near_start_threshold: f64,
    /// Fired (edge-triggered) when the scrolled window comes within
    /// `near_end_threshold` of content end — the "load newer" edge. See
    /// [`ListView::on_near_end`].
    on_near_end: Option<OnNearEnd<State>>,
    /// Distance from content end (px) at which `on_near_end` fires.
    near_end_threshold: f64,
    /// Fired on pointer `Up` when the past-top overscroll exceeded
    /// `REFRESH_TRIGGER_PX` (shared with [`crate::ScrollView`]). See
    /// [`ListView::on_refresh_release`].
    on_refresh_release: Option<OnRefresh<State>>,
    /// A custom [`ScrollPhysics`] installed via [`ListView::physics`], or
    /// `None` to leave whatever is already installed on the widget alone —
    /// mirrors [`crate::ScrollView`]'s field of the same name/contract; see
    /// [`ListView::physics`] for the full build/rebuild semantics.
    physics: Option<Rc<dyn ScrollPhysics>>,
    /// How past-edge pull is visualized, carried down to
    /// [`ListViewWidget::effect`] on every build/rebuild. See
    /// [`ListView::overscroll_effect`] (mirrors [`crate::ScrollView`]'s field
    /// of the same name).
    pub(crate) effect: OverscrollEffect,
}

impl<State: 'static> ListView<State> {
    /// Create a virtualized list of `item_count` rows, each `item_extent`
    /// logical pixels tall, whose row at `index` is produced by `builder`.
    ///
    /// The builder returns an [`AnyView`] (rows may differ in concrete view
    /// type); spell each row with [`frust_core::any`]. Panics if
    /// `item_extent` is not positive (the uniform extent is the virtualization
    /// fast path; a zero/negative extent has no well-defined window).
    ///
    /// # Contract
    ///
    /// Rows are reconciled by **raw item index**, not by a stable key. This is
    /// safe for append-only, truncate-only, and full-replace data, but a
    /// mid-list insert/remove/reorder silently reattaches a retained row's state
    /// to different content at the same index. See the [module docs]'
    /// *Row identity* section for the full rule, and
    /// [`ListView::builder_keyed`] for the stable-key alternative that survives
    /// a mid-list mutation.
    ///
    /// [module docs]: self
    pub fn builder(
        item_count: usize,
        item_extent: f64,
        builder: impl Fn(usize) -> AnyView<State> + 'static,
    ) -> Self {
        assert!(
            item_extent > 0.0,
            "ListView item_extent must be positive (uniform extent)"
        );
        Self {
            item_count,
            item_extent,
            builder: Rc::new(builder),
            key_of: None,
            estimated_item_extent: None,
            on_near_start: None,
            near_start_threshold: 0.0,
            on_near_end: None,
            near_end_threshold: 0.0,
            on_refresh_release: None,
            physics: None,
            effect: default_overscroll_effect(),
        }
    }

    /// Create a virtualized list whose rows are reconciled by the **stable key**
    /// `key_of(index)` instead of by raw item index — the mid-list-mutation-safe
    /// counterpart of [`ListView::builder`], everything else identical.
    ///
    /// `key_of` returns the identity of the row at an item index (a
    /// [`ChildKey`], built from any [`Hash`](std::hash::Hash) value — an item id,
    /// a name — via `ChildKey::new`/`.into()`); it is called once per
    /// *materialized* slot per frame, never `item_count` times, and must be a
    /// pure function of the index over one frame's data, exactly like `builder`.
    /// Panics if `item_extent` is not positive, like [`ListView::builder`].
    ///
    /// ```ignore
    /// ListView::builder_keyed(
    ///     rows.len(),
    ///     56.0,
    ///     move |i| ChildKey::new(rows[i].id),
    ///     move |i| any::<AppState, _>(row_view(&rows[i])),
    /// )
    /// ```
    ///
    /// # Contract
    ///
    /// A row whose key stays in the window keeps its live widget — and so its
    /// entire retained state — even when an insert/remove/reorder moves it to a
    /// different index; a key that leaves the window (or the data) is torn down,
    /// and a key entering is built fresh. **Keys must be unique within a
    /// window:** a duplicate trips a `debug_assert!` and, in release, hands the
    /// live row to the first slot claiming the key while later duplicates build
    /// fresh (no panic, but which row keeps its state is arbitrary). See the
    /// [module docs]' *Row identity* section.
    ///
    /// [module docs]: self
    pub fn builder_keyed(
        item_count: usize,
        item_extent: f64,
        key_of: impl Fn(usize) -> ChildKey + 'static,
        builder: impl Fn(usize) -> AnyView<State> + 'static,
    ) -> Self {
        assert!(
            item_extent > 0.0,
            "ListView item_extent must be positive (uniform extent)"
        );
        Self {
            item_count,
            item_extent,
            builder: Rc::new(builder),
            key_of: Some(Rc::new(key_of)),
            estimated_item_extent: None,
            on_near_start: None,
            near_start_threshold: 0.0,
            on_near_end: None,
            near_end_threshold: 0.0,
            on_refresh_release: None,
            physics: None,
            effect: default_overscroll_effect(),
        }
    }

    /// Let rows size themselves, taking `estimate_px` as the assumed extent of
    /// every row that has not been measured yet — **variable-extent mode**,
    /// available on [`ListView::builder_keyed`] lists only.
    ///
    /// A materialized row is laid out under the list's width with unbounded
    /// height and reports whatever height it wants; that height is cached under
    /// the row's stable key and used from then on for window math, the scroll
    /// extent, and the row's content position. Unmeasured rows (everything not
    /// yet laid out) count as `estimate_px`, so the scroll range converges on
    /// the true content height as the user visits rows. The constructor's
    /// `item_extent` is unused in this mode — pass the same value as the
    /// estimate for clarity.
    ///
    /// ```ignore
    /// ListView::builder_keyed(rows.len(), 72.0, key_of, builder)
    ///     .estimated_item_extent(72.0)
    /// ```
    ///
    /// # Contract
    ///
    /// **Keyed lists only.** A measured extent is cached under row identity, so
    /// it must move with the row; a positional list has no identity to cache
    /// under. Calling this on a [`ListView::builder`] list trips a
    /// `debug_assert!` and is **inert** in release — the list keeps its
    /// closed-form uniform extent rather than panicking live (the same shape as
    /// the duplicate-key tripwire, see the [module docs]' *Variable extents*
    /// section for why this is a `debug_assert` and not type-state). Panics if
    /// `estimate_px` is not positive, like the constructors' `item_extent`.
    ///
    /// [module docs]: self
    pub fn estimated_item_extent(mut self, estimate_px: f64) -> Self {
        assert!(
            estimate_px > 0.0,
            "ListView estimated_item_extent must be positive"
        );
        debug_assert!(self.key_of.is_some(), "{}", ESTIMATE_NEEDS_KEYS_MSG);
        self.estimated_item_extent = Some(estimate_px);
        self
    }

    /// The estimate this view actually runs in variable-extent mode with:
    /// `Some` only when a keyed list also named an estimate (the keyed-only
    /// contract's release behavior — see [`ListView::estimated_item_extent`]).
    fn variable_estimate(&self) -> Option<f64> {
        match (self.key_of.as_ref(), self.estimated_item_extent) {
            (Some(_), Some(estimate)) => Some(estimate),
            _ => None,
        }
    }

    /// Fire `callback` when the scrolled window comes within `threshold_px` of
    /// content **start** — the load-older edge for a newest-at-bottom chat list.
    ///
    /// The callback is **edge-triggered**: it fires once per approach and rearms
    /// only after the user scrolls away past `2 × threshold_px` (or the item
    /// count changes). Drag, wheel, and fling motion all observe it — fling
    /// motion is driven at paint time (no [`frust_core::EventCtx`]), so a
    /// fling-triggered fire is recorded and delivered on the next event, one
    /// event late; a `Cancel` clears a pending fire without invoking the
    /// callback.
    pub fn on_near_start<F: Fn(&mut State) + 'static>(
        mut self,
        callback: F,
        threshold_px: f64,
    ) -> Self {
        self.on_near_start = Some(Rc::new(callback));
        self.near_start_threshold = threshold_px;
        self
    }

    /// Fire `callback` when the scrolled window comes within `threshold_px` of
    /// content **end** — the load-newer edge for an infinite-scroll-downward
    /// list.
    ///
    /// Mirrors [`ListView::on_near_start`] exactly, measured from content end
    /// instead of start: **edge-triggered**, firing once per approach and
    /// rearming only after the user scrolls back past `2 × threshold_px` away
    /// from the end (or the item count changes). Drag, wheel, and fling motion
    /// all observe it — a fling-triggered fire is recorded and delivered on the
    /// next event, one event late; a `Cancel` clears a pending fire without
    /// invoking the callback.
    pub fn on_near_end<F: Fn(&mut State) + 'static>(
        mut self,
        callback: F,
        threshold_px: f64,
    ) -> Self {
        self.on_near_end = Some(Rc::new(callback));
        self.near_end_threshold = threshold_px;
        self
    }

    /// The pull-to-refresh trigger: fires on pointer `Up` when the list was
    /// pulled past the top by more than `REFRESH_TRIGGER_PX` (post-resistance)
    /// — the same name, signature, and threshold behavior as
    /// [`crate::ScrollView::on_refresh_release`], so a screen can swap between
    /// the two containers without relearning the contract. Never fires on a
    /// `Cancel`, and a bottom overscroll can never trigger it (only a past-top
    /// pull can). See the [module docs](self)' *Overscroll and pull-to-refresh*
    /// section.
    pub fn on_refresh_release<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_refresh_release = Some(Rc::new(callback));
        self
    }

    /// Install a custom [`ScrollPhysics`] strategy — the same seam
    /// [`crate::ScrollView::physics`] installs, sharing `scroll.rs`'s
    /// resistance/trigger/settle constants and this crate's
    /// [`crate::physics::parity`]/[`RubberBand`](crate::RubberBand)
    /// implementations.
    ///
    /// ```
    /// use frust_widgets::{ListView, NeverScrollable, text};
    /// let view: ListView<()> = ListView::builder(3, 40.0, |i| {
    ///     frust_core::any::<(), _>(text(i.to_string()))
    /// })
    /// .physics(NeverScrollable::new());
    /// # let _ = view;
    /// ```
    ///
    /// # Build/rebuild semantics
    ///
    /// Identical to [`crate::ScrollView::physics`]: a view built (or
    /// rebuilt) *with* `.physics(...)` reinstalls it on the widget every
    /// time; a view built (or rebuilt) *without* it leaves the widget's
    /// currently-installed physics untouched (a fresh `build` still starts at
    /// [`crate::physics::default_physics`], the widget's own constructor
    /// default).
    ///
    /// Defaults to the platform-adaptive physics
    /// ([`crate::physics::default_physics`] — Android clamping, elsewhere
    /// bouncing) if never called; `.physics(RubberBand::new())` is how an app
    /// asks for the pre-seam rubber-band feel instead.
    pub fn physics(mut self, physics: impl ScrollPhysics + 'static) -> Self {
        self.physics = Some(Rc::new(physics));
        self
    }

    /// Select how past-edge pull is visualized. See [`OverscrollEffect`]
    /// ([`crate::physics::effect`]) for the full contract — the same
    /// selector [`crate::ScrollView::overscroll_effect`] installs.
    ///
    /// ```
    /// use frust_widgets::{ListView, OverscrollEffect, text};
    /// let view: ListView<()> = ListView::builder(3, 40.0, |i| {
    ///     frust_core::any::<(), _>(text(i.to_string()))
    /// })
    /// .overscroll_effect(OverscrollEffect::Stretch);
    /// # let _ = view;
    /// ```
    ///
    /// Plain view-owned data, unlike [`ListView::physics`]: every
    /// build/rebuild carries the current value down to
    /// [`ListViewWidget::effect`] unconditionally.
    ///
    /// Defaults to the effect paired with the platform's default physics
    /// ([`crate::physics::default_overscroll_effect`]): the M3E
    /// [`OverscrollEffect::Stretch`] on Android, translate overscroll
    /// ([`OverscrollEffect::Translate`]) everywhere else.
    pub fn overscroll_effect(mut self, effect: OverscrollEffect) -> Self {
        self.effect = effect;
        self
    }
}

/// The reconciliation half of [`View::rebuild`], split per identity mode. See the
/// [module docs](self)' *Row identity* section for which one runs when.
impl<State: 'static> ListView<State> {
    /// Reconcile the materialized window to `[start, end)` **by item index** —
    /// the [`ListView::builder`] path, unchanged since the keyed one landed
    /// beside it: an index that stays in the window keeps its live pod and is
    /// rebuilt in place against its own previous view, indices leaving are torn
    /// down, indices entering are built fresh.
    ///
    /// Because the window is a contiguous range, any change to it necessarily
    /// builds or tears down at least one pod, so `structural` covers the
    /// "window shifted" case as well as the materialization one.
    fn reconcile_positional(
        &self,
        prev: &Self,
        element: &mut ListViewWidget,
        ctx: &mut BuildCtx<'_>,
        plan: WindowPlan,
    ) -> ChangeFlags {
        let (start, end) = (plan.start, plan.end);
        let mut flags = ChangeFlags::NONE;
        // Move the live window out so surviving indices can be relocated by key.
        let old_keys = std::mem::take(&mut element.keys);
        let old_children = std::mem::take(&mut element.children);
        let mut old: HashMap<usize, ChildPod> = old_keys.into_iter().zip(old_children).collect();

        let mut new_children = Vec::with_capacity(end.saturating_sub(start));
        let mut new_keys = Vec::with_capacity(end.saturating_sub(start));
        let mut structural = false;

        for index in start..end {
            if let Some(mut pod) = old.remove(&index) {
                // Survivor: rebuild in place against its own previous view
                // (reconstructed from the pure builder) — state preserved.
                let prev_view = (prev.builder)(index);
                let next_view = (self.builder)(index);
                flags |= crate::authoring::rebuild_child(&prev_view, &next_view, &mut pod, ctx);
                new_children.push(pod);
            } else {
                new_children.push(crate::authoring::build_child(&(self.builder)(index), ctx));
                structural = true;
            }
            new_keys.push(index);
        }

        // Indices that left the window are torn down (cancel-if-active inside
        // teardown_child unwinds an armed child).
        for (index, mut pod) in old.drain() {
            crate::authoring::teardown_child(&(prev.builder)(index), &mut pod, ctx);
            structural = true;
        }

        element.children = new_children;
        element.keys = new_keys;
        element.sync_child_origins();

        if structural {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    /// Reconcile the materialized window to `[start, end)` **by stable key** —
    /// the [`ListView::builder_keyed`] path.
    ///
    /// Slots stay the contiguous index run `[start, end)` (the uniform-extent
    /// fast path is untouched); identity is matched through the retained
    /// [`ListViewWidget::key_index`] map instead. For each slot: the key
    /// `key_of(index)` names the item index that row occupied last frame, whose
    /// live pod is relocated into this slot and rebuilt against
    /// `prev.builder(prev_index)` → `self.builder(index)`. Keys with no entry
    /// build fresh; pods no slot claimed are torn down with the same
    /// cancel-if-active [`crate::authoring::teardown_child`] path the positional
    /// reconciler uses — nothing more.
    ///
    /// A duplicate key is ambiguous and trips a `debug_assert!` mirroring
    /// [`crate::authoring::rebuild_children`]'s; release builds carry on (first
    /// claim keeps the live row, later duplicates build fresh) rather than panic.
    ///
    /// In variable-extent mode this pass additionally retains the window's slot
    /// keys (so `layout` can cache each measurement under the right identity
    /// without re-keying), re-pins the prefix anchor from `plan`, and applies
    /// the departing-row half of the measured cache's hygiene rules — `delta` is
    /// this frame's net `item_count` change, the second hypothesis of the same
    /// probe [`ListView::anchor_shift_items`] uses. `anchor_probe_missed` is
    /// that same probe's own outcome (`true` when it matched no previous-window
    /// key at all); this pass is where the wholesale-replace half of the
    /// hygiene rules is actually decided, against its own exact per-slot key
    /// matches below rather than the probe's hypotheses — see the [module
    /// docs](self)' *Variable extents* and *Cache hygiene* sections.
    #[allow(clippy::too_many_arguments)]
    fn reconcile_keyed(
        &self,
        prev: &Self,
        element: &mut ListViewWidget,
        ctx: &mut BuildCtx<'_>,
        plan: WindowPlan,
        key_of: &KeyOf,
        delta: isize,
        anchor_probe_missed: bool,
    ) -> ChangeFlags {
        let (start, end) = (plan.start, plan.end);
        let capacity = end.saturating_sub(start);
        // One pass over the new window: each slot's key (so `key_of` runs exactly
        // once per slot), the map this frame will retain, and the duplicate check
        // — done up front, like the authoring reconciler's, so a debug build trips
        // before any pod has been moved.
        let mut slot_keys: Vec<ChildKey> = Vec::with_capacity(capacity);
        let mut next_index_of: HashMap<ChildKey, usize> = HashMap::with_capacity(capacity);
        let mut duplicate = false;
        for index in start..end {
            let key = key_of(index);
            duplicate |= next_index_of.insert(key, index).is_some();
            slot_keys.push(key);
        }
        debug_assert!(!duplicate, "{}", DUPLICATE_KEY_MSG);

        // Move the live window out so surviving rows can be relocated by key: the
        // previous frame's `key -> item index` map, and the pods by the item index
        // each rendered.
        let prev_index_of = std::mem::take(&mut element.key_index);
        let old_keys = std::mem::take(&mut element.keys);
        let old_children = std::mem::take(&mut element.children);
        // A moved window range owes a layout pass on its own, without depending on
        // the match outcome below to prove it (the window is a contiguous run, so
        // first index + length pin it exactly).
        let window_shifted = old_keys.first().copied() != (capacity > 0).then_some(start)
            || old_keys.len() != capacity;
        let mut old: HashMap<usize, ChildPod> = old_keys.into_iter().zip(old_children).collect();

        let mut new_children = Vec::with_capacity(capacity);
        let mut new_keys = Vec::with_capacity(capacity);
        let mut flags = ChangeFlags::NONE;
        let mut structural = window_shifted;
        // Whether any slot in this frame's reconciled window matched a key the
        // previous window also held — an *exact* hashmap lookup, unlike the
        // rebuild-time anchor-shift probe's two index hypotheses. Read below to
        // decide the wholesale-replace half of the measured-cache hygiene
        // rules.
        let mut any_survivor = false;

        for (index, key) in (start..end).zip(slot_keys.iter().copied()) {
            let survivor = prev_index_of
                .get(&key)
                .copied()
                .and_then(|prev_index| old.remove(&prev_index).map(|pod| (prev_index, pod)));
            match survivor {
                Some((prev_index, mut pod)) => {
                    any_survivor = true;
                    // The row survived under its key: relocate its live pod into
                    // this slot and rebuild it in place against the view it
                    // actually holds — `prev.builder(prev_index)`, the index it
                    // rendered last frame. That redirection is the whole point of
                    // the retained map; state (hosted component, press, toggle)
                    // rides along with the pod.
                    let prev_view = (prev.builder)(prev_index);
                    let next_view = (self.builder)(index);
                    flags |= crate::authoring::rebuild_child(&prev_view, &next_view, &mut pod, ctx);
                    if prev_index != index {
                        // Same row, different slot: its origin moves, so the frame
                        // owes a layout pass even though nothing was built or torn
                        // down.
                        structural = true;
                    }
                    new_children.push(pod);
                }
                None => {
                    // A key with no live row: entering the window, or new data.
                    new_children.push(crate::authoring::build_child(&(self.builder)(index), ctx));
                    structural = true;
                }
            }
            new_keys.push(index);
        }

        // Measured-cache hygiene (variable extents only).
        if element.is_variable() {
            if anchor_probe_missed && !any_survivor {
                // The rebuild-time anchor-shift probe matched no previous-window
                // key by hypothesis, *and* literally no previous-window key
                // matched a slot in this frame's reconciled window by exact
                // lookup either — a genuine full replace (or a jump far enough
                // that nothing on screen is recognizable), which is reset
                // semantics for the measured cache too. Deliberately not
                // triggered by `anchor_probe_missed` alone: that probe only
                // tests two candidate index shifts and can miss on a same-frame
                // mutation touching both sides of the anchor (a prepend above
                // the viewport plus an append below it in one frame) while
                // `any_survivor` above still proves most on-screen rows are
                // exactly the ones they were — see the [module docs](self)'
                // *Cache hygiene* section.
                element.clear_measured();
            } else {
                // A pod no slot claimed left the window *or* the data, and only
                // the second case may drop its measurement. Probe each orphan's
                // key at its unchanged index and at that index shifted by this
                // frame's net item-count delta — the same two hypotheses
                // anchoring uses, at most two `key_of` calls per departing row,
                // never a scan of `item_count`. Same acknowledged imprecision as
                // the anchor probe (see the module docs' *Cache hygiene*
                // section's gaps) — narrower blast radius than the wholesale
                // clear above, since a false miss here only evicts one
                // already-departed row's entry.
                //
                // Runs on every frame that isn't a genuine full replace,
                // including one where the anchor-shift probe above missed
                // (`anchor_probe_missed && any_survivor`, the branch that lands
                // here rather than in the wholesale clear above): a probe miss
                // does not mean this loop's own two hypotheses are unreliable
                // for a *specific* orphaned row — it only means no *single*
                // uniform hypothesis explained every row in the previous
                // window at once (the same-frame both-sides-of-the-anchor
                // case). Gating this loop on the probe's outcome, as an
                // earlier version of this fix did, traded a bounded
                // over-eviction (re-measure one row that was merely outside
                // the still-mis-anchored window) for an unbounded leak: a row
                // truly removed from the data on such a frame has no later
                // shrink or revisit to reclaim it — `record_measurement` never
                // refreshes a dead key, and `evict_measured_stale_indices`
                // only scans on a frame whose `item_count` shrank, which a
                // frame that *adds* a sentinel/anchor row alongside a removal
                // (any_survivor's own precondition) need never be. A false
                // eviction here costs one row's re-measurement — the module's
                // own documented conservative direction — never a permanent
                // leak.
                let count = element.item_count;
                for (key, prev_index) in prev_index_of.iter() {
                    if old.contains_key(prev_index)
                        && !Self::key_survives(*key, *prev_index, delta, count, key_of)
                    {
                        element.forget_measured(key);
                    }
                }
            }
        }

        // Every pod no slot claimed is a key that left the window or the data.
        for (index, mut pod) in old.drain() {
            crate::authoring::teardown_child(&(prev.builder)(index), &mut pod, ctx);
            structural = true;
        }

        element.children = new_children;
        element.keys = new_keys;
        element.key_index = next_index_of;
        if let Some(estimate) = element.variable_estimate() {
            // Retain this window's identities beside its slots, then re-pin the
            // prefix anchor and the per-slot content positions to the plan the
            // window was computed from (layout refines them from measurements).
            element.slot_keys = slot_keys;
            element.set_window_geometry(plan, estimate);
        }
        element.sync_child_origins();

        if structural {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    /// Determine the keyed prepend/removal scroll-anchor correction for this
    /// rebuild, in **items** (the caller multiplies by `item_extent`), or
    /// `None` if no correction should apply. See the [module docs](self)'
    /// anchoring section for the algorithm and the `offset == 0` decision.
    ///
    /// `element.item_count` must already hold this frame's (new) count —
    /// callers apply the item-count bookkeeping before calling this — while
    /// `old_item_count` is the count as of the *previous* frame.
    fn anchor_shift_items(
        prev: &Self,
        element: &ListViewWidget,
        old_item_count: usize,
        key_of: &KeyOf,
    ) -> Option<isize> {
        // A prior frame built with the positional path (or no key_of at all)
        // leaves no previous-frame key function to reconstruct an anchor
        // key from — nothing to anchor against.
        let prev_key_of = prev.key_of.as_ref()?;
        let new_item_count = element.item_count;
        let delta = new_item_count as isize - old_item_count as isize;
        for &i_prev in &element.keys {
            let anchor_key = prev_key_of(i_prev);
            // Unchanged position: the below-viewport/no-op case (the common
            // case — checked first, resolves in one extra `key_of` call).
            if i_prev < new_item_count && key_of(i_prev) == anchor_key {
                return Some(0);
            }
            // Shifted by the frame's net item-count delta: the
            // prepend/removal-above case, confirmed rather than assumed.
            if delta != 0 {
                let candidate = i_prev as isize + delta;
                if candidate >= 0
                    && (candidate as usize) < new_item_count
                    && key_of(candidate as usize) == anchor_key
                {
                    return Some(delta);
                }
            }
        }
        // No key in the previous window survived either hypothesis: a full
        // replace (or an edit this closed-form check can't characterize) —
        // reset semantics, not an anchoring case.
        None
    }

    /// Whether `key` still identifies a row in this frame's data, probed at the
    /// index it last occupied and at that index shifted by the frame's net
    /// item-count `delta` — the same two hypotheses
    /// [`ListView::anchor_shift_items`] tests, at most two `key_of` calls. Used
    /// by the measured cache's departing-row eviction; a `false` here means the
    /// row left the data, not merely the window.
    fn key_survives(
        key: ChildKey,
        prev_index: usize,
        delta: isize,
        item_count: usize,
        key_of: &KeyOf,
    ) -> bool {
        if prev_index < item_count && key_of(prev_index) == key {
            return true;
        }
        if delta != 0 {
            let candidate = prev_index as isize + delta;
            if candidate >= 0
                && (candidate as usize) < item_count
                && key_of(candidate as usize) == key
            {
                return true;
            }
        }
        false
    }
}

/// Create a virtualized [`ListView`] — the free-function spelling of
/// [`ListView::builder`].
pub fn list_view<State: 'static>(
    item_count: usize,
    item_extent: f64,
    builder: impl Fn(usize) -> AnyView<State> + 'static,
) -> ListView<State> {
    ListView::builder(item_count, item_extent, builder)
}

/// The retained widget for a [`ListView`]: the materialized window of children
/// (`children[j]` renders item `keys[j]`), the scroll offset + cached viewport,
/// and the same fling bookkeeping as [`crate::ScrollWidget`].
pub struct ListViewWidget {
    /// The currently materialized rows, parallel to [`ListViewWidget::keys`].
    children: Vec<ChildPod>,
    /// The item index each entry of [`ListViewWidget::children`] renders. Always
    /// a contiguous ascending run (uniform extent → the window is a range).
    keys: Vec<usize>,
    /// Row identity for a [`ListView::builder_keyed`] list: `stable key -> the
    /// item index that row occupied in the materialized window`, as of the last
    /// build/rebuild. Empty for a positional [`ListView::builder`] list (and
    /// cleared by any positional rebuild, so a list switched between the two
    /// constructors never matches against a window the other path materialized).
    ///
    /// This is the retained half of keyed reconciliation: window slots stay
    /// contiguous indices for the uniform-extent fast path, and identity is
    /// carried here instead — read back by `rebuild` off the element exactly like
    /// [`ListViewWidget::offset`]/[`ListViewWidget::viewport`], since the view is
    /// reconstructed from scratch every frame.
    key_index: HashMap<ChildKey, usize>,
    /// The keyed list's own `index -> identity` function, retained (a cheap
    /// [`Rc`] clone, reinstalled every rebuild like the erased callbacks) so the
    /// widget's own passes can key an index without the view in scope: the
    /// variable-extent prefix walk looks a row's measurement up by key, and
    /// `paint`'s convergence check re-derives the same window. `None` for a
    /// positional list. This is the *key* function only — the builder is never
    /// retained here, and never runs outside rebuild (see the [module
    /// docs](self)' *Variable extents* section).
    key_of: Option<KeyOf>,
    /// The assumed extent of an unmeasured row in variable-extent mode, or
    /// `None` for the closed-form uniform path. Set from
    /// [`ListView::estimated_item_extent`], already resolved against the
    /// keyed-only contract.
    estimated_extent: Option<f64>,
    /// Variable-extent mode: every row this widget has ever laid out, by stable
    /// key. Bounded by the keys a session has visited — see the [module
    /// docs](self)' cache-hygiene rules (and the LRU deferral noted there).
    measured: HashMap<ChildKey, Measured>,
    /// The running Σ of [`ListViewWidget::measured`]'s extents, so the content
    /// extent is O(1) rather than a scan of the cache.
    measured_sum: f64,
    /// Variable-extent mode: the identity of each materialized slot, parallel to
    /// [`ListViewWidget::keys`], so `layout` can cache a measurement under the
    /// right key without calling `key_of` again. Empty on the uniform path.
    slot_keys: Vec<ChildKey>,
    /// Variable-extent mode: each materialized slot's content-space `y`,
    /// parallel to [`ListViewWidget::keys`] — what rows are placed at (minus the
    /// offset) instead of the uniform path's `index * item_extent`. Empty on the
    /// uniform path.
    slot_y: Vec<f64>,
    /// Variable-extent mode: the prefix anchor's item index — the item
    /// [`ListViewWidget::anchor_y`] gives the content-space top of, and the
    /// point every per-frame prefix walk starts from (the previous window's
    /// start). See the [module docs](self)' *Variable extents* section.
    anchor_index: usize,
    /// The content-space `y` of [`ListViewWidget::anchor_index`]'s top.
    anchor_y: f64,
    /// Variable-extent mode: the scroll correction `layout` has measured but no
    /// rebuild has committed into [`ListViewWidget::offset`] yet — Σ
    /// `measured − assumed` over the rows that lay wholly above the viewport top
    /// in the geometry the frame's window was planned against. Always `0.0` on
    /// the uniform path (which measures nothing). Read back through
    /// [`ListViewWidget::placement_offset`] by everything that places content, so
    /// it is honored the frame it is recorded; committed by
    /// [`ListViewWidget::apply_pending_correction`]. See the [module docs](self)'
    /// *Measured anchor correction* section.
    pending_correction: f64,
    item_count: usize,
    item_extent: f64,
    /// The **windowing** scroll offset — always in `[0, max_offset]`, the
    /// value every item-index computation (window planning, the prefix walk,
    /// edge triggers) reads through [`ListViewWidget::placement_offset`]. Never
    /// carries an out-of-range value; see [`ListViewWidget::overscroll`] and
    /// the [module docs](self)' *Overscroll and pull-to-refresh* section for
    /// where a drag past an edge is represented instead.
    offset: f64,
    /// The physics-mapped drag position accumulated during an active scroll
    /// drag, **before** boundary rejection, in the same **raw-offset space**
    /// [`ListViewWidget::offset`] itself lives in — never
    /// [`ListViewWidget::placement_offset`]'s pending-corrected space, since
    /// [`ListViewWidget::apply_drag_offset`] splits it straight into `offset`
    /// plus `overscroll` by absolute assignment. Seeded at takeover from
    /// `offset + overscroll` (the raw offset plus the live displacement — the
    /// intentional regrab-mid-bounce term, so a regrab mid-bounce continues
    /// smoothly from what is on screen) and advanced by each move's mapped
    /// delta thereafter; deliberately **not**
    /// [`ListViewWidget::painted_offset`], which would also fold in a nonzero
    /// [`ListViewWidget::pending_correction`] and double-count it once
    /// `placement_offset` re-adds it on the first post-takeover move.
    /// Mirrors `ScrollWidget::drag_position`, including the part that runs off
    /// past an edge under a clamping physics.
    drag_position: f64,
    /// The signed, resisted past-edge visual displacement a drag shows beyond
    /// the windowing [`ListViewWidget::offset`]: negative past the top,
    /// positive past the bottom, `0.0` while in range. Only a drag ever sets
    /// this nonzero; wheel and fling always force it back to `0.0` (both stay
    /// hard-clamped, matching `ScrollView`). See
    /// [`ListViewWidget::painted_offset`] and the [module docs](self)'
    /// *Overscroll and pull-to-refresh* section.
    overscroll: f64,
    /// Whether a release-settle animation is easing an overscrolled
    /// [`ListViewWidget::overscroll`] back to `0.0` (driven at paint via
    /// [`ListViewWidget::settle_tick`], mirrors `ScrollWidget::settling`).
    settling: bool,
    /// The installed scroll physics — [`crate::physics::default_physics`]'s
    /// platform-adaptive choice unless [`ListView::physics`] replaces it, the
    /// same default `ScrollView` takes. `Rc`, not
    /// `Box`: every [`ScrollPhysics`] method takes `&self`, so a shared,
    /// immutable handle is both cheap to (re)install and sufficient (mirrors
    /// `ScrollWidget::physics` — see its doc for the full rationale).
    /// Survives a rebuild whose view carries no `.physics(...)` call
    /// untouched; see [`ListView::physics`] for the full contract and the
    /// [module docs](self)' *Overscroll and pull-to-refresh* section.
    pub(crate) physics: Rc<dyn ScrollPhysics>,
    /// How past-edge pull is visualized —
    /// [`crate::physics::default_overscroll_effect`]'s platform pairing unless
    /// [`ListView::overscroll_effect`] names one. Read at paint alone (see
    /// [`ListViewWidget::painted_offset`] and `scroll.rs`'s *Overscroll
    /// visuals*), never by layout, windowing, or the physics.
    pub(crate) effect: OverscrollEffect,
    /// The signed pull past an edge, negative past the top: the displacement
    /// the physics allowed ([`ListViewWidget::overscroll`]) plus whatever
    /// [`ScrollPhysics::apply_boundary_conditions`] rejected while the position
    /// was pinned at the edge. Under a physics that rejects nothing
    /// (`Bouncing`, [`RubberBand`](crate::RubberBand)) it *is* `overscroll` —
    /// which is why basing the refresh trigger on it changes no trigger
    /// distance for either. Same field, same contract, same sign as
    /// `ScrollWidget::edge_pull`; see it for the full rule.
    pub(crate) edge_pull: f64,
    /// A generic ballistic simulation handed back by
    /// [`ScrollPhysics::create_ballistic_simulation`] on release, or `None` —
    /// always `None` under [`RubberBand`](crate::RubberBand), which keeps the
    /// legacy [`ListViewWidget::fling`] path instead.
    ballistic: Option<BallisticState>,
    /// Velocity (px/s of offset) of the motion a new `Down` interrupted, fed to
    /// [`ScrollPhysics::carried_momentum`] at the next fling start. Always
    /// `0.0` when the press landed on a resting surface.
    carried_velocity: f64,
    /// Resolved viewport size (this widget's own size), cached from the previous
    /// layout so rebuild can window against it (BuildCtx carries no viewport).
    viewport: Size,
    /// Whether a scroll drag has taken the gesture over (past the slop).
    scrolling: bool,
    /// Whether a `Down` armed an active gesture (mirrors `ScrollWidget`).
    down_active: bool,
    /// What the nearest nested scroll surface inside a materialized row claimed
    /// it could do with this gesture, read back out of the claim cell this
    /// widget pushed around the `Down`'s routing — the input to the
    /// innermost-wins decision at the takeover site. Same field, same contract,
    /// same `Down`-time staleness window as `ScrollWidget::inner_at_down`; see
    /// it for the full rule.
    pub(crate) inner_at_down: InnerScrollState,
    /// Whether this gesture was handed to that nested surface — sticky for the
    /// rest of the gesture, exactly like `ScrollWidget::deferring`.
    pub(crate) deferring: bool,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    /// Active fling velocity (px/s of offset), or `None` when not flinging.
    fling: Option<f64>,
    /// Last painted frame time, reused as the event-pass timestamp for velocity
    /// tracking (the event pass carries no clock).
    last_frame_time: FrameTime,
    /// Last animation frame time for the paint-time fling pump; `None` seeds the
    /// clock (zero-delta) on the first paint after a release.
    last_anim: Option<FrameTime>,
    /// The near-start "load older" callback (`None` if the view set none).
    on_near_start: Option<ErasedCallback>,
    /// Distance from content start (px) at which `on_near_start` fires.
    near_start_threshold: f64,
    /// Whether `on_near_start` is armed to fire on the next near-start approach.
    /// Cleared when it fires; rearmed after scrolling away past `2 × threshold`
    /// or an item-count change.
    near_start_armed: bool,
    /// A near-start fire detected by the paint-time fling pump (no `EventCtx`);
    /// delivered on the next event, cleared by a `Cancel` without firing.
    pending_near_start: bool,
    /// The near-end "load newer" callback (`None` if the view set none).
    on_near_end: Option<ErasedCallback>,
    /// Distance from content end (px) at which `on_near_end` fires.
    near_end_threshold: f64,
    /// Whether `on_near_end` is armed to fire on the next near-end approach.
    /// Cleared when it fires; rearmed after scrolling away past `2 ×
    /// threshold` or an item-count change.
    near_end_armed: bool,
    /// A near-end fire detected by the paint-time fling pump (no `EventCtx`);
    /// delivered on the next event, cleared by a `Cancel` without firing.
    pending_near_end: bool,
    /// The pull-to-refresh release callback (`None` if the view set none).
    on_refresh_release: Option<ErasedCallback>,
}

impl ListViewWidget {
    fn new(item_count: usize, item_extent: f64) -> Self {
        Self {
            children: Vec::new(),
            keys: Vec::new(),
            key_index: HashMap::new(),
            key_of: None,
            estimated_extent: None,
            measured: HashMap::new(),
            measured_sum: 0.0,
            slot_keys: Vec::new(),
            slot_y: Vec::new(),
            anchor_index: 0,
            anchor_y: 0.0,
            pending_correction: 0.0,
            item_count,
            item_extent,
            offset: 0.0,
            drag_position: 0.0,
            overscroll: 0.0,
            settling: false,
            physics: default_physics(),
            effect: default_overscroll_effect(),
            edge_pull: 0.0,
            ballistic: None,
            carried_velocity: 0.0,
            viewport: Size::ZERO,
            scrolling: false,
            down_active: false,
            inner_at_down: InnerScrollState::default(),
            deferring: false,
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
            on_near_start: None,
            near_start_threshold: 0.0,
            near_start_armed: true,
            pending_near_start: false,
            on_near_end: None,
            near_end_threshold: 0.0,
            near_end_armed: true,
            pending_near_end: false,
            on_refresh_release: None,
        }
    }

    /// Edge-detect the near-start "load older" condition, mutating the armed
    /// state: rearm once scrolled away past `2 × threshold`, and return `true`
    /// exactly once when armed and the offset comes within `threshold` of content
    /// start. Callers fire the callback on a `true` return.
    ///
    /// Measured against [`ListViewWidget::placement_offset`] — where the content
    /// actually sits — rather than the raw offset, so an uncommitted
    /// variable-extent correction can neither rearm nor fire this edge (the
    /// placement is exactly what a commit leaves unchanged). Identical to the
    /// offset on the uniform path, which never has a pending correction.
    fn evaluate_near_start(&mut self) -> bool {
        if self.on_near_start.is_none() || self.item_count == 0 {
            return false;
        }
        let threshold = self.near_start_threshold;
        let position = self.placement_offset();
        if position > 2.0 * threshold {
            self.near_start_armed = true;
        }
        if self.near_start_armed && position <= threshold {
            self.near_start_armed = false;
            return true;
        }
        false
    }

    /// Fire `on_near_start` during the event pass if the near-start edge just
    /// triggered.
    fn fire_near_start(&mut self, ctx: &mut EventCtx) {
        if self.evaluate_near_start()
            && let Some(cb) = self.on_near_start.as_mut()
        {
            cb(ctx);
        }
    }

    /// Deliver a near-start fire recorded by the paint-time fling pump on the
    /// next event (one event of latency — the paint pass has no `EventCtx`).
    fn deliver_pending_near_start(&mut self, ctx: &mut EventCtx) {
        if self.pending_near_start {
            self.pending_near_start = false;
            if let Some(cb) = self.on_near_start.as_mut() {
                cb(ctx);
            }
        }
    }

    /// Edge-detect the near-end "load newer" condition, mirroring
    /// [`Self::evaluate_near_start`] but measured from content **end**: the
    /// remaining scrollable distance below the viewport (`max_offset − offset`)
    /// rather than the offset itself. Reads the placement for the same reason
    /// [`Self::evaluate_near_start`] does.
    fn evaluate_near_end(&mut self) -> bool {
        if self.on_near_end.is_none() || self.item_count == 0 {
            return false;
        }
        let threshold = self.near_end_threshold;
        let distance = self.max_offset() - self.placement_offset();
        if distance > 2.0 * threshold {
            self.near_end_armed = true;
        }
        if self.near_end_armed && distance <= threshold {
            self.near_end_armed = false;
            return true;
        }
        false
    }

    /// Fire `on_near_end` during the event pass if the near-end edge just
    /// triggered.
    fn fire_near_end(&mut self, ctx: &mut EventCtx) {
        if self.evaluate_near_end()
            && let Some(cb) = self.on_near_end.as_mut()
        {
            cb(ctx);
        }
    }

    /// Deliver a near-end fire recorded by the paint-time fling pump on the
    /// next event (mirrors [`Self::deliver_pending_near_start`]).
    fn deliver_pending_near_end(&mut self, ctx: &mut EventCtx) {
        if self.pending_near_end {
            self.pending_near_end = false;
            if let Some(cb) = self.on_near_end.as_mut() {
                cb(ctx);
            }
        }
    }

    /// The current scroll offset.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The item indices currently materialized (the visible window ± buffer).
    pub fn window(&self) -> &[usize] {
        &self.keys
    }

    /// The maximum scroll offset (`extent − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.content_extent() - self.viewport.height).max(0.0)
    }

    /// The content-space `y` the viewport's top edge is at this frame **for
    /// windowing purposes**: the committed [`ListViewWidget::offset`] plus the
    /// correction `layout` has measured and no rebuild has committed yet,
    /// through the same `[0, max_offset]` clamp every offset write takes —
    /// always in range, never carrying [`ListViewWidget::overscroll`].
    ///
    /// Everything that decides item indices reads this — the prefix walk, the
    /// window, each row's content-space `y`, the near-start/near-end edge
    /// triggers — so a measured correction is honored the instant it is
    /// recorded and committing it is visually a no-op, and a row is never
    /// asked to materialize for an out-of-range index. Everything that *moves*
    /// the scroll (drag, wheel, fling, clamp) works on the raw offset instead.
    /// Identical to the offset on the uniform path, which never measures and so
    /// never has a pending correction. See the [module docs](self)' *Measured
    /// anchor correction* and *Overscroll and pull-to-refresh* sections; for
    /// where content is actually **painted** (which does layer overscroll on
    /// top), see [`ListViewWidget::painted_offset`].
    fn placement_offset(&self) -> f64 {
        if self.pending_correction == 0.0 {
            return self.offset;
        }
        (self.offset + self.pending_correction).clamp(0.0, self.max_offset())
    }

    /// The content-space `y` the viewport's top edge is actually **painted**
    /// at this frame: [`ListViewWidget::placement_offset`] (always in range)
    /// plus the current [`ListViewWidget::overscroll`] displacement (`0.0`
    /// outside a drag past an edge). Read by
    /// [`ListViewWidget::sync_child_origins`] and [`Widget::semantics`]'s
    /// scroll position — the only two places the out-of-range number is ever
    /// allowed to show, so the content edge visually displaces while no row
    /// is ever asked to materialize outside `[0, item_count)`. See the
    /// [module docs](self)' *Overscroll and pull-to-refresh* section.
    ///
    /// The displacement is layered in only under
    /// [`OverscrollEffect::Translate`]: [`OverscrollEffect::Stretch`] paints
    /// the pull as a scale about the held edge instead (see
    /// [`Widget::paint`]) and [`OverscrollEffect::None`] paints it not at all,
    /// so under both the content — and the position semantics reports for it —
    /// stays exactly where the in-range windowing offset puts it. `overscroll`
    /// itself still evolves identically under every effect; only this read
    /// differs (`scroll.rs`'s *Overscroll visuals*).
    fn painted_offset(&self) -> f64 {
        match self.effect {
            OverscrollEffect::Translate => self.placement_offset() + self.overscroll,
            OverscrollEffect::Stretch | OverscrollEffect::None => self.placement_offset(),
        }
    }

    /// Commit the pending measured correction into [`ListViewWidget::offset`] —
    /// the one place it is ever committed, from [`View::rebuild`] before the
    /// window is planned. Returns whether the offset actually moved (a
    /// correction the clamp swallowed whole reports `false`: nothing changed, so
    /// nothing is owed a layout pass).
    ///
    /// **Withheld while a fling is live**: the pump is advancing the offset at
    /// paint, and a correction landing on top of that can walk it backwards or
    /// onto a bound that ends the fling early, so the sum keeps accumulating and
    /// the first rebuild after the fling stops commits all of it (see the
    /// [module docs](self)' *Measured anchor correction* section).
    fn apply_pending_correction(&mut self) -> bool {
        if self.pending_correction == 0.0 || self.is_flinging() {
            return false;
        }
        let target = self.offset + self.pending_correction;
        self.pending_correction = 0.0;
        let before = self.offset;
        self.set_offset(target);
        self.offset != before
    }

    /// The whole content's extent: `item_count * item_extent` exactly on the
    /// uniform path, or `Σ measured + estimate × unmeasured` in variable-extent
    /// mode (O(1) — the sum is maintained as rows are measured). See the [module
    /// docs](self)' *Variable extents* section.
    fn content_extent(&self) -> f64 {
        match self.variable_estimate() {
            Some(estimate) => {
                let unmeasured = self.item_count.saturating_sub(self.measured.len());
                self.measured_sum + estimate * unmeasured as f64
            }
            None => self.item_count as f64 * self.item_extent,
        }
    }

    /// Whether post-release motion is in flight — the legacy fling, or a
    /// physics-supplied [`Simulation`] the generic driver is running (never
    /// both, and never either one under [`RubberBand`](crate::RubberBand)'s
    /// legacy-only path).
    pub fn is_flinging(&self) -> bool {
        self.fling.is_some() || self.ballistic.is_some()
    }

    /// This surface's extent/position snapshot for the physics, reading
    /// `pixels` from a caller-supplied position rather than a field: the drag
    /// path asks about the *raw* (un-resisted) drag position clamped into
    /// range, so resistance is derived from the accumulator instead of
    /// compounding across moves. `max_scroll_extent` is
    /// [`ListViewWidget::max_offset`], read fresh, so a still-converging
    /// variable-extent content extent is always what the physics sees.
    fn metrics_at(&self, pixels: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: self.max_offset(),
            viewport_dimension: self.viewport.height,
            device_pixel_ratio: METRICS_FALLBACK_DPR,
        }
    }

    /// This surface's extent/position snapshot at the scroll position a physics
    /// reasons about: the **raw** windowing offset plus any live displacement,
    /// which is the same space [`ListViewWidget::max_offset`] and
    /// [`ListViewWidget::drag_raw`] live in. Deliberately not
    /// [`ListViewWidget::painted_offset`], which also folds in an uncommitted
    /// [`ListViewWidget::pending_correction`] — a physics answer fed back into
    /// raw space would double-count it, exactly as it would at drag takeover.
    fn metrics(&self) -> ScrollMetrics {
        self.metrics_at(self.offset + self.overscroll)
    }

    /// The velocity (px/s of offset) of whatever post-release motion is live
    /// right now — the legacy fling's own, or a running simulation's at the
    /// last painted frame — and `0.0` when the list is at rest.
    fn live_velocity(&self) -> f64 {
        if let Some(v) = self.fling {
            return v;
        }
        match self.ballistic.as_ref() {
            Some(state) => state.sim.dx(state.elapsed_secs(self.last_frame_time)),
            None => 0.0,
        }
    }

    /// A new fling's starting velocity: the `release` velocity plus whatever
    /// [`ScrollPhysics::carried_momentum`] carries over from the motion this
    /// gesture's `Down` interrupted (`0.0` under
    /// [`RubberBand`](crate::RubberBand), leaving `release` untouched).
    /// Mirrors `ScrollWidget::fling_start_velocity`, gate included: momentum
    /// is carried only onto a release that plainly continues the interrupted
    /// motion — same sign, and faster than
    /// [`MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR`] of the physics' own
    /// **mapped** share of the interrupted velocity —
    /// `physics.carried_momentum(carried)`, the exact value the release is
    /// about to add, not the raw interrupted speed (see that method's doc
    /// for why this matters) — since the carried term is comparable in
    /// magnitude to an ordinary release and would otherwise cancel out or
    /// reverse a flick back the other way. Flutter's third guard, dropping
    /// the carried velocity when the finger held still before letting go, is
    /// an accepted gap here too (see that method for the full contract).
    fn fling_start_velocity(&self, release: f64) -> f64 {
        let mapped = self.physics.carried_momentum(self.carried_velocity);
        let continues_it = release.signum() == mapped.signum()
            && release.abs() > MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped.abs();
        if continues_it {
            release + mapped
        } else {
            release
        }
    }

    /// Ask the installed physics for post-release motion, bounded by its own
    /// [`ScrollPhysics::min_fling_velocity`]/[`ScrollPhysics::max_fling_velocity`]
    /// — **the generic driver's bounds only**; the legacy fling below keeps its
    /// pinned `FLING_STOP` threshold and no upper clamp. Mirrors
    /// `ScrollWidget::release_simulation`.
    fn release_simulation(&self) -> Option<Box<dyn Simulation>> {
        // The offset moves opposite the finger, like every other release path.
        let released = self.fling_start_velocity(-self.tracker.velocity());
        let max = self.physics.max_fling_velocity();
        let velocity = if released.abs() < self.physics.min_fling_velocity() {
            0.0
        } else {
            released.clamp(-max, max)
        };
        self.physics
            .create_ballistic_simulation(&self.metrics(), velocity)
    }

    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    fn clamp_offset(&mut self) {
        self.set_offset(self.offset);
    }

    /// Advance the drag by `delta` px of raw finger travel in offset space
    /// (positive = the content scrolls down) and re-split the result across the
    /// windowing [`ListViewWidget::offset`], the
    /// [`ListViewWidget::overscroll`] displacement and
    /// [`ListViewWidget::edge_pull`], mirroring
    /// [`crate::ScrollWidget::apply_drag_offset`] (see `scroll.rs`'s *Drag
    /// convention* for why the physics is handed a per-move delta at the live
    /// position rather than a whole excursion from a clamped base).
    ///
    /// The one thing local to this widget: because its windowing offset may
    /// never leave range (the [module docs](self)' *Overscroll and
    /// pull-to-refresh* section), the position the physics allows is clamped
    /// into `offset` and whatever is left over lands in `overscroll` for paint
    /// alone. `max_offset` is read fresh every call, so a variable-extent
    /// list's still-converging content extent is always what that split runs
    /// against.
    fn apply_drag_offset(&mut self, delta: f64) {
        let metrics = self.metrics();
        let mapped = self.physics.apply_physics_to_user_offset(&metrics, delta);
        self.drag_position += mapped;
        let rejected = self
            .physics
            .apply_boundary_conditions(&metrics, self.drag_position);
        let allowed = self.drag_position - rejected;
        self.offset = allowed.clamp(0.0, self.max_offset());
        self.overscroll = allowed - self.offset;
        self.edge_pull = self.overscroll + rejected;
    }

    /// Advance a release-settle by `dt_ms`, easing
    /// [`ListViewWidget::overscroll`] back to `0.0` and returning whether it
    /// is still animating. Pure and deterministic (mirrors
    /// [`crate::ScrollWidget::settle_tick`]); the paint pump and the tests
    /// both drive it. Only ever eases `overscroll` — the windowing
    /// [`ListViewWidget::offset`] already sits at the exact edge (`0.0` or
    /// `max_offset`) the moment a drag overscrolls, so it needs no motion of
    /// its own here.
    pub fn settle_tick(&mut self, dt_ms: f64) -> bool {
        if !self.settling {
            return false;
        }
        // `edge_pull`'s boundary-rejected half (always `0.0` under
        // `RubberBand`, where the pull *is* the displacement) has no
        // displacement to ride back, so it decays on the same curve of its own
        // — otherwise a clamping physics' stretch would snap off at release.
        let rejected = self.edge_pull - self.overscroll;
        if self.overscroll.abs() <= SETTLE_STOP_PX && rejected.abs() <= SETTLE_STOP_PX {
            self.overscroll = 0.0;
            self.edge_pull = 0.0;
            self.settling = false;
            self.sync_child_origins();
            return false;
        }
        let retained = SETTLE_DECAY.powf(dt_ms);
        self.overscroll *= retained;
        self.edge_pull = self.overscroll + rejected * retained;
        self.sync_child_origins();
        true
    }

    /// The `[start, end)` item range that should be materialized for the current
    /// offset + cached viewport, clamped to `[0, item_count]`, plus the
    /// content-space `y` of `start`. With a zero viewport (first build) this is
    /// the conservative initial window.
    ///
    /// Variable-extent mode takes the prefix-walk path instead; the uniform
    /// closed form below is unchanged.
    fn desired_window(&self) -> WindowPlan {
        if let Some(estimate) = self.variable_estimate() {
            return if self.item_count == 0 {
                WindowPlan::EMPTY
            } else {
                self.desired_window_variable(estimate)
            };
        }
        if self.item_count == 0 || self.item_extent <= 0.0 {
            return WindowPlan::EMPTY;
        }
        let vh = self.viewport.height;
        let first = (self.offset / self.item_extent).floor() as isize - BUFFER;
        let last = ((self.offset + vh) / self.item_extent).ceil() as isize + BUFFER;
        let start = first.max(0) as usize;
        let end = (last.max(0) as usize).min(self.item_count);
        let start = start.min(end);
        WindowPlan {
            start,
            end,
            y_start: start as f64 * self.item_extent,
        }
    }

    /// Whether the materialized window fully covers `[start, end)`.
    fn window_covers(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return true;
        }
        match (self.keys.first(), self.keys.last()) {
            (Some(&f), Some(&l)) => f <= start && l + 1 >= end,
            _ => false,
        }
    }

    /// Place each materialized row at its content position minus the
    /// **painted** offset (row `i` occupies `y ∈ [i*extent, (i+1)*extent)` in
    /// content space) — [`ListViewWidget::painted_offset`], not the windowing
    /// one, so an overscrolled drag's out-of-range displacement shows on
    /// screen even though [`ListViewWidget::offset`] itself never leaves
    /// `[0, max_offset]`.
    ///
    /// Variable-extent mode places each row at its own retained content `y`
    /// instead — the same rule, over the prefix walk's positions rather than the
    /// closed form.
    fn sync_child_origins(&mut self) {
        let painted = self.painted_offset();
        if self.is_variable() {
            for (y, pod) in self.slot_y.iter().zip(self.children.iter_mut()) {
                pod.set_origin(Point::new(0.0, *y - painted));
            }
            return;
        }
        let extent = self.item_extent;
        for (index, pod) in self.keys.iter().zip(self.children.iter_mut()) {
            pod.set_origin(Point::new(0.0, *index as f64 * extent - painted));
        }
    }

    // --- Variable extents (keyed lists only). See the module docs' *Variable
    //     extents* section; every method here is inert on the uniform path. ---

    /// The estimate this widget is running variable-extent mode with, or `None`
    /// for the closed-form uniform path. Both halves are required: identity to
    /// cache a measurement under, and an estimate for the rows without one.
    fn variable_estimate(&self) -> Option<f64> {
        match (self.key_of.as_ref(), self.estimated_extent) {
            (Some(_), Some(estimate)) => Some(estimate),
            _ => None,
        }
    }

    /// Whether variable-extent mode is active (see
    /// [`ListViewWidget::variable_estimate`]).
    fn is_variable(&self) -> bool {
        self.variable_estimate().is_some()
    }

    /// The extent one *unmeasured* row contributes: the estimate in
    /// variable-extent mode, the uniform `item_extent` otherwise. The scroll
    /// anchoring correction steps by this (a prepended row is by definition
    /// unmeasured).
    fn unmeasured_extent(&self) -> f64 {
        self.variable_estimate().unwrap_or(self.item_extent)
    }

    /// The extent item `index` contributes: its cached measurement if it has
    /// one, else the estimate. One `key_of` call, no builder call.
    fn extent_at(&self, index: usize, estimate: f64) -> f64 {
        let Some(key_of) = self.key_of.as_ref() else {
            return estimate;
        };
        if index >= self.item_count {
            return estimate;
        }
        self.measured
            .get(&key_of(index))
            .map(|m| m.extent)
            .unwrap_or(estimate)
    }

    /// Walk the retained prefix anchor to the current offset, returning the
    /// first item the offset falls inside and that item's content-space top.
    ///
    /// O(step): a scroll/fling frame moves a few items; a jump farther than
    /// [`MAX_PREFIX_STEP`] items resolves its bulk in closed form against the
    /// estimate first. Reaching item 0 re-pins `y = 0` exactly, erasing any
    /// floating-point drift the walk accumulated.
    fn walk_to_offset(&self, estimate: f64) -> (usize, f64) {
        let count = self.item_count;
        debug_assert!(count > 0, "walk_to_offset needs a non-empty list");
        let target = self.placement_offset();
        let mut index = self.anchor_index.min(count - 1);
        let mut y = self.anchor_y;

        let gap = target - y;
        if gap.abs() > estimate * MAX_PREFIX_STEP as f64 {
            // A data reset or a programmatic jump, never a scroll: cover the
            // bulk against the estimate so the walk below stays bounded.
            let jump = (gap / estimate).trunc();
            let landed = (index as f64 + jump).clamp(0.0, (count - 1) as f64);
            y += (landed - index as f64) * estimate;
            index = landed as usize;
        }

        let mut steps = 0usize;
        while y > target && index > 0 && steps < MAX_PREFIX_STEP {
            index -= 1;
            y -= self.extent_at(index, estimate);
            steps += 1;
        }
        while index + 1 < count && steps < MAX_PREFIX_STEP {
            let extent = self.extent_at(index, estimate);
            if y + extent <= target {
                y += extent;
                index += 1;
                steps += 1;
            } else {
                break;
            }
        }
        if index == 0 {
            y = 0.0;
        }
        (index, y)
    }

    /// The variable-extent counterpart of [`ListViewWidget::desired_window`]:
    /// the item the offset falls inside, widened by [`BUFFER`] above and by
    /// whatever it takes to cover the viewport (plus [`BUFFER`]) below.
    /// O(window + step).
    fn desired_window_variable(&self, estimate: f64) -> WindowPlan {
        let count = self.item_count;
        let (first_visible, y_visible) = self.walk_to_offset(estimate);

        // Leading buffer: back up from the first visible item, accumulating the
        // same extents in reverse.
        let mut start = first_visible;
        let mut y_start = y_visible;
        for _ in 0..BUFFER {
            if start == 0 {
                break;
            }
            start -= 1;
            y_start -= self.extent_at(start, estimate);
        }
        if start == 0 {
            y_start = 0.0;
        }

        // Forward to the viewport's bottom edge, then the trailing buffer.
        let bottom = self.placement_offset() + self.viewport.height;
        let mut end = first_visible + 1;
        let mut y_end = y_visible + self.extent_at(first_visible, estimate);
        while end < count && y_end < bottom {
            y_end += self.extent_at(end, estimate);
            end += 1;
        }
        let end = end.saturating_add(BUFFER as usize).min(count);
        WindowPlan {
            start,
            end: end.max(start),
            y_start,
        }
    }

    /// Re-pin the prefix anchor to the window `plan` this frame materialized and
    /// refill the per-slot content positions from the cached extents. Layout
    /// refines those positions from the rows' actual measurements.
    fn set_window_geometry(&mut self, plan: WindowPlan, estimate: f64) {
        self.anchor_index = plan.start;
        self.anchor_y = if plan.start == 0 { 0.0 } else { plan.y_start };
        let keys = std::mem::take(&mut self.keys);
        let mut ys = std::mem::take(&mut self.slot_y);
        ys.clear();
        let mut y = self.anchor_y;
        for &index in &keys {
            ys.push(y);
            y += self.extent_at(index, estimate);
        }
        self.slot_y = ys;
        self.keys = keys;
    }

    /// Cache one row's laid-out extent under its stable key, keeping the running
    /// sum exact.
    fn record_measurement(&mut self, key: ChildKey, index: usize, extent: f64) {
        match self.measured.insert(key, Measured { index, extent }) {
            Some(previous) => self.measured_sum += extent - previous.extent,
            None => self.measured_sum += extent,
        }
    }

    /// Drop one row's measurement (its key left the data).
    fn forget_measured(&mut self, key: &ChildKey) {
        if let Some(previous) = self.measured.remove(key) {
            self.measured_sum -= previous.extent;
        }
    }

    /// Drop every measurement — a full replace, where no previous key survives.
    /// Any uncommitted correction goes with them: it describes rows this list no
    /// longer holds, so committing it into new data would be a guess.
    fn clear_measured(&mut self) {
        self.measured.clear();
        self.measured_sum = 0.0;
        self.pending_correction = 0.0;
    }

    /// Drop measurements the current keying provably cannot produce any more:
    /// entries recorded at an index past the (just shrunk) end. A scan of the
    /// cache, run only on a frame whose `item_count` shrank. A stale recorded
    /// index can evict a still-live row early; it is re-measured on its next
    /// visit, which is the conservative direction.
    fn evict_measured_stale_indices(&mut self) {
        if self.measured.is_empty() {
            return;
        }
        let count = self.item_count;
        let mut dropped = 0.0;
        self.measured.retain(|_, entry| {
            let live = entry.index < count;
            if !live {
                dropped += entry.extent;
            }
            live
        });
        self.measured_sum -= dropped;
    }

    /// Drop every variable-extent artifact: the measured cache, the per-slot
    /// identities and positions, and the prefix anchor. Run when a list leaves
    /// variable-extent mode, so nothing stale can be read back if it re-enters.
    fn reset_variable_state(&mut self) {
        self.clear_measured();
        self.slot_keys.clear();
        self.slot_y.clear();
        self.anchor_index = 0;
        self.anchor_y = 0.0;
    }

    /// Drop every keyed-identity artifact: the `key -> index` map plus all
    /// variable-extent bookkeeping (which is cached under those identities). Run
    /// when a positional frame rebuilds a list a keyed frame materialized (see
    /// [`View::rebuild`]).
    fn reset_keyed_state(&mut self) {
        self.key_index.clear();
        self.reset_variable_state();
    }

    /// Apply a confirmed scroll-anchor shift of `items` rows: the offset absorbs
    /// it, and (in variable-extent mode) so does the prefix anchor, which names
    /// an item index that just moved by the same amount.
    fn apply_anchor_shift(&mut self, items: isize) {
        let step = self.unmeasured_extent();
        self.set_offset(self.offset + items as f64 * step);
        if self.is_variable() {
            let index = (self.anchor_index as isize + items).max(0) as usize;
            self.anchor_y = if index == 0 {
                0.0
            } else {
                (self.anchor_y + items as f64 * step).max(0.0)
            };
            self.anchor_index = index;
        }
    }

    /// Lay the materialized rows out under a bounded width and **unbounded**
    /// height (the [`crate::ScrollView`] precedent), caching each row's chosen
    /// height under its key and re-deriving the window's content positions from
    /// those heights. Measures only pods that already exist — no builder call,
    /// no materialization (the wake hazard the module docs call out).
    ///
    /// It also records — never applies — the frame's anchor correction: two
    /// running positions are carried, one over the heights the rows actually
    /// chose and one over the extents this frame's window was *planned*
    /// against, and every row whose planned span lies wholly above the viewport
    /// top contributes its `measured − assumed` delta to
    /// [`ListViewWidget::pending_correction`]. Those are exactly the rows whose
    /// mis-estimate moves the anchor row, and the offset owes their sum back.
    /// The offset itself is never written here beyond the pre-existing range
    /// clamp (the viewport is only known at layout) — the correction is
    /// committed by the next [`View::rebuild`]. See the [module docs](self)'
    /// *Measured anchor correction* section.
    fn layout_variable(&mut self, ctx: &mut LayoutCtx, vw: f64, estimate: f64) {
        let child_bc = BoxConstraints::new(Size::new(vw, 0.0), Size::new(vw, f64::INFINITY));
        // The viewport's top edge in content space, in the geometry this frame's
        // window was planned against — the line that decides which rows sit
        // above the anchor.
        let top = self.placement_offset();
        let mut children = std::mem::take(&mut self.children);
        let mut ys = std::mem::take(&mut self.slot_y);
        ys.clear();
        let mut y = self.anchor_y;
        let mut y_assumed = self.anchor_y;
        let mut correction = 0.0;
        for (slot, pod) in children.iter_mut().enumerate() {
            let key = self.slot_keys.get(slot).copied();
            let index = self.keys.get(slot).copied();
            // Read the assumed extent *before* this row's own measurement is
            // recorded below, or the delta would always be zero.
            let assumed = index.map_or(estimate, |index| self.extent_at(index, estimate));
            let size = pod.layout_child(ctx, &child_bc);
            ys.push(y);
            if y_assumed + assumed <= top {
                correction += size.height - assumed;
            }
            y += size.height;
            y_assumed += assumed;
            if let (Some(key), Some(index)) = (key, index) {
                self.record_measurement(key, index, size.height);
            }
        }
        self.slot_y = ys;
        self.children = children;
        // Accumulate, never assign: a correction withheld across a live fling is
        // still owed. Re-measuring an unchanged row contributes a zero delta, so
        // this can never double-count one.
        self.pending_correction += correction;
        // The measurements just revised the content extent (the estimate still
        // governs everything outside the window), so re-clamp before the caller
        // syncs origins.
        self.clamp_offset();
    }

    /// Advance an in-flight fling by `dt_ms`, returning whether it is still
    /// animating. Pure and deterministic — the paint-time pump and the tests
    /// both drive it (mirrors [`crate::ScrollWidget::tick`]).
    pub fn tick(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.fling else {
            return false;
        };
        self.set_offset(self.offset + fling_displacement(v, dt_ms));
        self.sync_child_origins();
        let next_v = fling_decay(v, dt_ms);
        let at_bound = self.offset <= 0.0 || self.offset >= self.max_offset();
        if next_v.abs() < FLING_STOP || at_bound {
            self.fling = None;
            false
        } else {
            self.fling = Some(next_v);
            true
        }
    }

    /// Advance a physics-supplied [`Simulation`] to frame time `now`, the
    /// windowing/displacement split preserved: the position it reports, minus
    /// whatever [`ScrollPhysics::apply_boundary_conditions`] rejects of it,
    /// clamped into [`ListViewWidget::offset`] with the remainder left in
    /// [`ListViewWidget::overscroll`] where paint (never the item-index math)
    /// reads it. Subtracting the rejection is what keeps the driver honest for
    /// **any** physics — a clamping one can never displace even if its
    /// simulation overshoots, while a bouncing one (rejecting nothing) is free
    /// to run past the edge and back. Mirrors
    /// [`crate::ScrollWidget::drive_ballistic`], including its early stop for a
    /// curve that is [pinned
    /// outward](ListViewWidget::ballistic_is_pinned_outward) and the handoff of
    /// any leftover pull to the settle
    /// ([`ListViewWidget::settle_ballistic_residual`]).
    fn drive_ballistic(&mut self, now: FrameTime) {
        let Some((proposed, velocity, done)) = self.ballistic.as_ref().map(|state| {
            let t = state.elapsed_secs(now);
            (state.sim.x(t), state.sim.dx(t), state.sim.is_done(t))
        }) else {
            return;
        };
        let rejected = self
            .physics
            .apply_boundary_conditions(&self.metrics(), proposed);
        let allowed = proposed - rejected;
        self.offset = allowed.clamp(0.0, self.max_offset());
        self.overscroll = allowed - self.offset;
        self.edge_pull = self.overscroll + rejected;
        self.sync_child_origins();
        if done || self.ballistic_is_pinned_outward(proposed, rejected, velocity) {
            self.ballistic = None;
            self.settle_ballistic_residual();
        }
    }

    /// Whether the running simulation can no longer move anything on screen —
    /// its proposal is outside the range, the physics rejected **all** of that
    /// excess, and the curve is still travelling further out. See
    /// [`crate::ScrollWidget::ballistic_is_pinned_outward`] for the full
    /// contract and why the test is deliberately conservative.
    ///
    /// One caveat local to this widget: `max_offset` is a *converging* estimate
    /// in variable-extent mode, so a fling stopped here against an
    /// under-estimated end stays stopped rather than resuming when later
    /// measurements push the end out. Accepted — the offset is already pinned
    /// at that estimated end for as long as the estimate holds, so the
    /// difference is a fling that ends where the surface had already stopped
    /// moving.
    fn ballistic_is_pinned_outward(&self, proposed: f64, rejected: f64, velocity: f64) -> bool {
        let excess = proposed - proposed.clamp(0.0, self.max_offset());
        excess != 0.0 && rejected == excess && velocity * excess > 0.0
    }

    /// Hand a residual [`ListViewWidget::edge_pull`] left behind by a finished
    /// ballistic to the release-settle, so it decays on [`SETTLE_DECAY`]
    /// exactly as a drag release's pull does instead of standing on screen
    /// until the next `Down`/wheel/`Cancel`. Mirrors
    /// [`crate::ScrollWidget::settle_ballistic_residual`], guard and all.
    fn settle_ballistic_residual(&mut self) {
        if self.edge_pull.abs() > SETTLE_STOP_PX {
            self.settling = true;
        }
    }

    /// Advance the ballistic simulation, the legacy fling, *or* the
    /// release-settle by the delta since the last paint, and signal
    /// [`PaintCtx::request_frame`] while any is still
    /// running (mirrors [`crate::ScrollWidget::pump_fling`]). The continuation
    /// frame is what re-runs the shell's rebuild → the window re-materializes
    /// as the fling carries on; a settle never changes the window (it only
    /// eases [`ListViewWidget::overscroll`], which windowing never reads), so
    /// it needs the request purely to keep painting the animation.
    ///
    /// The three are mutually exclusive by construction — a release picks one —
    /// and under [`RubberBand`](crate::RubberBand) the simulation arm is never
    /// taken at all.
    fn pump_fling(&mut self, ctx: &mut PaintCtx) {
        if self.fling.is_none() && !self.settling && self.ballistic.is_none() {
            self.last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let dt = match self.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_anim = Some(now);
        // A simulation measures time from its own start, so the first pump
        // after the release seeds it — the same zero-delta seeding frame
        // `last_anim` takes, so neither clock ever jumps on frame one.
        if let Some(state) = self.ballistic.as_mut() {
            state.start.get_or_insert(now);
        }
        if dt > 0.0 {
            if self.ballistic.is_some() {
                self.drive_ballistic(now);
            } else if self.fling.is_some() {
                self.tick(dt);
            } else {
                self.settle_tick(dt);
            }
            // The fling moved the offset with no `EventCtx` in scope; if it
            // crossed the near-start/near-end edge, record the fire for the
            // next event. (A settle never moves `placement_offset()`, so this
            // is a no-op there, not a special case worth branching out.)
            if self.evaluate_near_start() {
                self.pending_near_start = true;
            }
            if self.evaluate_near_end() {
                self.pending_near_end = true;
            }
        }
        if self.fling.is_some() || self.settling || self.ballistic.is_some() {
            ctx.request_frame();
        }
    }

    /// Deliver a synthetic `Cancel` to whichever child holds the capture path,
    /// disarming an armed `ListItem` press when the scroll drag takes over.
    fn cancel_children(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        crate::authoring::route_event(&mut self.children, ctx, &cancel);
    }

    /// The event body, parameterised on an explicit timestamp so velocity math
    /// is deterministic in tests; [`Widget::event`] supplies the real clock.
    /// Adapted from [`crate::ScrollWidget`], routing to the *window* of children
    /// via [`crate::authoring::route_event`] rather than a single child.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        // A fling-driven near-start fire recorded at paint time is delivered on
        // the next event — except a Cancel, which clears it without firing.
        if !matches!(
            event,
            InputEvent::Pointer(p) if p.phase == PointerPhase::Cancel
        ) {
            self.deliver_pending_near_start(ctx);
            self.deliver_pending_near_end(ctx);
        }
        match event {
            // A broadcast reaches every realized child unconditionally and is
            // never consumed — `route_event` owns that contract, so this arm just
            // hands it over ahead of the gesture machinery.
            InputEvent::Housekeeping => {
                crate::authoring::route_event(&mut self.children, ctx, event)
            }
            InputEvent::Key(_) | InputEvent::Ime(_) => {
                crate::authoring::route_event(&mut self.children, ctx, event)
            }
            InputEvent::Scroll { delta, .. } => {
                let dy = match delta {
                    ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                // Wheel scrolling stays hard-clamped — no overscroll rubber-band
                // on wheel input, matching `ScrollView`, and no physics
                // consulted: the clamp is a property of the input device, not
                // of the installed feel, so this arm is identical under every
                // physics.
                self.fling = None;
                self.settling = false;
                self.ballistic = None;
                self.overscroll = 0.0;
                self.edge_pull = 0.0;
                self.set_offset(self.offset + dy);
                self.sync_child_origins();
                self.fire_near_start(ctx);
                self.fire_near_end(ctx);
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    // Only a primary press arms a drag. A secondary press is a
                    // context gesture: it still reaches the realized rows (a
                    // context-menu consumer in a row must see it), but opens no
                    // capture and can never start a scroll — the `ScrollView`
                    // rule, applied to the windowing list.
                    if !presses(p) {
                        return crate::authoring::route_event(&mut self.children, ctx, event);
                    }
                    self.scrolling = false;
                    self.down_active = true;
                    self.inner_at_down = InnerScrollState::default();
                    self.deferring = false;
                    // Remember what this press interrupted before killing it —
                    // the next fling asks the physics how much of it to carry
                    // forward (`0.0` under `RubberBand`, i.e. start cold).
                    self.carried_velocity = self.live_velocity();
                    self.fling = None;
                    self.settling = false;
                    self.ballistic = None;
                    // A `Down` deliberately leaves a mid-bounce displacement on
                    // screen (the regrab continues from it), so the pull is
                    // re-seeded from that displacement rather than zeroed —
                    // "reset" here means "carries nothing stale from the
                    // previous gesture".
                    self.edge_pull = self.overscroll;
                    self.last_anim = None;
                    self.down_start = p.position;
                    self.last_drag = p.position;
                    self.tracker.clear();
                    self.tracker.record(t_ms, p.position.y);
                    ctx.capture_pointer();
                    // Innermost-wins arbitration, in dispatch order: report
                    // THIS list into whatever cell is ambient (the nearest
                    // enclosing scrollable's, if any) while that is still the
                    // top of the stack, then push this list's own cell for the
                    // routing below so a row's nested scrollable writes here
                    // rather than past this level. `scroll.rs` owns the seam;
                    // this is the second consumer of it, not a second copy.
                    if let Some(host) = ambient_scroll_claim() {
                        host.set(inner_claim_state(self.physics.as_ref(), &self.metrics()));
                    }
                    let claim = Rc::new(Cell::new(InnerScrollState::default()));
                    let children = &mut self.children;
                    with_scroll_claim(&claim, || {
                        crate::authoring::route_event(children, ctx, event)
                    });
                    self.inner_at_down = claim.get();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if !self.down_active {
                        return crate::authoring::route_event(&mut self.children, ctx, event);
                    }
                    self.tracker.record(t_ms, p.position.y);
                    if self.scrolling {
                        let dy = p.position.y - self.last_drag.y;
                        self.last_drag = p.position;
                        // Hand the physics this move's raw delta (the offset
                        // moves opposite the finger); whatever it makes of a
                        // past-edge pull lands in `overscroll`, never in a
                        // windowing offset outside `[0, max_offset]` (see the
                        // module docs' *Overscroll and pull-to-refresh*).
                        self.apply_drag_offset(-dy);
                        self.sync_child_origins();
                        self.fire_near_start(ctx);
                        self.fire_near_end(ctx);
                        ctx.request_redraw();
                    } else if !self.deferring
                        && (p.position.y - self.down_start.y).abs() > TOUCH_SLOP
                        && self.physics.should_accept_user_offset(&self.metrics())
                    {
                        if self.inner_at_down.defers(p.position.y - self.down_start.y) {
                            // Innermost wins: a nested scrollable inside a row
                            // registered on this gesture's `Down` and can
                            // consume this direction, so take nothing over —
                            // no `Cancel` to the rows, no capture handover —
                            // and keep routing. Sticky for the rest of the
                            // gesture (`deferring` gates this whole branch);
                            // the inner's own slop machinery takes it from
                            // here. See the module docs' *Nested scrolling*.
                            self.deferring = true;
                            crate::authoring::route_event(&mut self.children, ctx, event);
                        } else {
                            // Take the gesture over: cancel the armed child,
                            // stop forwarding — the documented window-shift
                            // capture-loss tradeoff's sibling. A physics that
                            // refuses drags outright keeps the move flowing to
                            // the row instead; `RubberBand` accepts
                            // unconditionally (even content that fits
                            // rubber-bands), so that gate is inert on the
                            // default feel.
                            self.scrolling = true;
                            self.settling = false;
                            self.last_drag = p.position;
                            // Seed the drag accumulator from the raw offset
                            // plus any live overscroll — never
                            // `painted_offset`, which also folds in
                            // `pending_correction`: `apply_drag_offset` splits
                            // `drag_position` straight into `offset` by
                            // absolute assignment, and `placement_offset`
                            // unconditionally re-adds `pending_correction` on
                            // top, so seeding from painted space would
                            // double-count a nonzero pending correction on the
                            // first post-takeover move. Including `overscroll`
                            // (not just `offset`) is the intentional
                            // regrab-mid-bounce term, so a regrab mid-bounce
                            // still continues smoothly from what is on screen.
                            self.drag_position = self.offset + self.overscroll;
                            self.cancel_children(ctx, p.position);
                            ctx.request_redraw();
                        }
                    } else {
                        crate::authoring::route_event(&mut self.children, ctx, event);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if self.scrolling {
                        // Pull-to-refresh: released past the top trigger fires the
                        // app hook (an Up, so mutating state is allowed). Shares
                        // the exact threshold check `ScrollView` uses — see the
                        // module docs' *Overscroll and pull-to-refresh* section
                        // — measured on `edge_pull`, which under `RubberBand` is
                        // bit-for-bit the overscroll it has always read.
                        if crossed_refresh_trigger(self.edge_pull)
                            && let Some(cb) = self.on_refresh_release.as_mut()
                        {
                            cb(ctx);
                        }
                        // Ask the physics for post-release motion first: one
                        // that hands back a simulation owns the release
                        // outright, and one that does not (`RubberBand`) falls
                        // through to the legacy settle/fling below untouched.
                        if let Some(sim) = self.release_simulation() {
                            self.fling = None;
                            self.settling = false;
                            self.ballistic = Some(BallisticState { sim, start: None });
                            self.last_anim = None;
                        } else if self.edge_pull != 0.0 {
                            // Released while overscrolled: settle back to the
                            // edge, never fling out of range.
                            self.fling = None;
                            self.settling = true;
                            self.last_anim = None;
                        } else {
                            // The legacy path keeps its own FLING_STOP threshold
                            // (the trait's min/max fling bounds govern the
                            // generic driver only) and takes carried momentum,
                            // which is `0.0` under `RubberBand`.
                            let finger_v = self.tracker.velocity();
                            if finger_v.abs() > FLING_STOP {
                                self.fling = Some(self.fling_start_velocity(-finger_v));
                                self.last_anim = None;
                            }
                        }
                    } else {
                        crate::authoring::route_event(&mut self.children, ctx, event);
                    }
                    self.scrolling = false;
                    self.down_active = false;
                    self.inner_at_down = InnerScrollState::default();
                    self.deferring = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    crate::authoring::route_event(&mut self.children, ctx, event);
                    self.scrolling = false;
                    self.down_active = false;
                    self.inner_at_down = InnerScrollState::default();
                    self.deferring = false;
                    // Cancel never fires a callback and snaps any overscroll away
                    // with no settle animation: drop any pending
                    // near-start/near-end fire without invoking it, and never
                    // fires on_refresh_release (mirrors `ScrollWidget`'s Cancel).
                    self.pending_near_start = false;
                    self.pending_near_end = false;
                    self.settling = false;
                    self.ballistic = None;
                    self.overscroll = 0.0;
                    self.edge_pull = 0.0;
                    self.sync_child_origins();
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
        }
    }
}

impl<State: 'static> View<State> for ListView<State> {
    type Element = ListViewWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ListViewWidget {
        let mut widget = ListViewWidget::new(self.item_count, self.item_extent);
        widget.on_near_start = self
            .on_near_start
            .as_ref()
            .map(crate::authoring::erase_callback);
        widget.near_start_threshold = self.near_start_threshold;
        widget.on_near_end = self
            .on_near_end
            .as_ref()
            .map(crate::authoring::erase_callback);
        widget.near_end_threshold = self.near_end_threshold;
        widget.on_refresh_release = self
            .on_refresh_release
            .as_ref()
            .map(crate::authoring::erase_callback);
        if let Some(physics) = self.physics.clone() {
            widget.physics = physics;
        }
        widget.effect = self.effect;
        // The key function and the unmeasured-row estimate are widget state (the
        // window math and layout's measurement both run without the view in
        // scope), and must be installed before the first window is planned.
        widget.key_of = self.key_of.clone();
        widget.estimated_extent = self.variable_estimate();
        // Conservative initial window from a zero viewport (converges within one
        // extra frame via paint's continuation request — see the module docs).
        let plan = widget.desired_window();
        let variable = widget.is_variable();
        let mut duplicate = false;
        for index in plan.start..plan.end {
            widget
                .children
                .push(crate::authoring::build_child(&(self.builder)(index), ctx));
            widget.keys.push(index);
            if let Some(key_of) = self.key_of.as_ref() {
                // Seed the retained identity map so the first rebuild can already
                // relocate these rows by key.
                let key = key_of(index);
                duplicate |= widget.key_index.insert(key, index).is_some();
                if variable {
                    widget.slot_keys.push(key);
                }
            }
        }
        debug_assert!(!duplicate, "{}", DUPLICATE_KEY_MSG);
        if let Some(estimate) = widget.variable_estimate() {
            widget.set_window_geometry(plan, estimate);
        }
        widget.sync_child_origins();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ListViewWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the erased adapter.
        element.on_near_start = self
            .on_near_start
            .as_ref()
            .map(crate::authoring::erase_callback);
        element.near_start_threshold = self.near_start_threshold;
        element.on_near_end = self
            .on_near_end
            .as_ref()
            .map(crate::authoring::erase_callback);
        element.near_end_threshold = self.near_end_threshold;
        element.on_refresh_release = self
            .on_refresh_release
            .as_ref()
            .map(crate::authoring::erase_callback);
        // A `.physics(...)`-carrying view reinstalls it every rebuild, like the
        // erased callbacks above; a view with no opinion (`None`) leaves the
        // widget's currently-installed physics alone — see `ListView::physics`'s
        // doc for the full contract.
        if let Some(physics) = self.physics.clone() {
            element.physics = physics;
        }
        // The visual effect is plain data the view owns and is always carried
        // down unconditionally.
        element.effect = self.effect;
        // Closures are not comparable either — reinstall the key function the
        // widget's own passes key indices with (never the builder; see the
        // module docs' *Variable extents* section).
        element.key_of = self.key_of.clone();

        let mut flags = ChangeFlags::NONE;
        let old_item_count = element.item_count;
        let mut item_count_changed = false;
        if element.item_count != self.item_count {
            element.item_count = self.item_count;
            item_count_changed = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.item_extent != self.item_extent {
            element.item_extent = self.item_extent;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let estimate = self.variable_estimate();
        if element.estimated_extent != estimate {
            // Entering, leaving, or re-scaling variable-extent mode is an
            // extent-affecting change like `item_extent`: every row's position
            // is derived from it. Leaving it also drops the bookkeeping, so a
            // later re-entry starts from measurements this list actually took.
            element.estimated_extent = estimate;
            if estimate.is_none() {
                element.reset_variable_state();
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Prepend/removal scroll anchoring — keyed lists only (positional
        // identity cannot distinguish a prepend from a full mutation, see the
        // module docs' *Row identity* section): correct the offset for the
        // topmost surviving keyed row of the previous window *before* this
        // frame's window is recomputed below, so layout and paint agree on
        // one corrected offset — never during paint. See the module docs'
        // anchoring section for the algorithm and the `offset == 0` decision.
        let mut prepend_correction = false;
        // Set when the anchor-shift hypothesis probe below misses on every key
        // in the previous window. NOT itself proof of a full replace — the
        // probe only tests two candidate index shifts and can miss on a
        // same-frame mutation touching both sides of the anchor (a prepend
        // above the viewport plus an append below it in one frame) even
        // though most on-screen rows are still exactly the ones they were.
        // Whether the measured *cache* should reset on that is answered
        // later, inside `reconcile_keyed`, against its own exact per-slot key
        // matches — see the module docs' *Cache hygiene* section. The
        // *pending measured-anchor correction*, in contrast, is zeroed
        // unconditionally the moment the probe misses (below) — a narrower,
        // unrelated question the cache-reset decision does not gate.
        let mut anchor_probe_missed = false;
        if let Some(key_of) = self.key_of.as_ref() {
            match Self::anchor_shift_items(prev, element, old_item_count, key_of) {
                Some(shift_items) if shift_items != 0 => {
                    element.apply_anchor_shift(shift_items);
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    prepend_correction = shift_items > 0;
                }
                Some(_) => {}
                None => {
                    anchor_probe_missed = true;
                    // A pending measured-anchor correction describes rows
                    // wholly above the viewport top *in the geometry this
                    // frame's now-superseded window was planned against*
                    // (module docs' *Measured anchor correction* section).
                    // The anchor probe just proved that geometry cannot be
                    // explained against this mutation by either hypothesis —
                    // committing the accumulated correction into new data
                    // below (`apply_pending_correction`, called after this
                    // match) would be a guess, not a measurement. Zero it
                    // here, before that commit point, regardless of whether
                    // the measured *cache* itself resets: that is a separate,
                    // narrower decision made later in `reconcile_keyed`
                    // against its own exact per-slot key matches (see the
                    // module docs' *Cache hygiene* section) — a probe miss
                    // with a surviving on-screen row keeps the cache but
                    // still owes this reset, since the correction was
                    // computed against the pre-mutation window regardless of
                    // whether any individual row's measurement survives.
                    element.pending_correction = 0.0;
                }
            }
        }

        if item_count_changed {
            // Content length changed — rearm the near-start "load older" and
            // near-end "load newer" edges so a list that grew (older rows
            // loaded, or newer rows appended) can trigger again — EXCEPT: a
            // keyed prepend correction just proved this exact growth was the
            // near-start edge's own fire being answered, so forcing it back
            // armed here would let the very next scroll event refire it
            // immediately, at the exact anchored offset the correction just
            // placed the user at (see the module docs' anchoring section).
            if !prepend_correction {
                element.near_start_armed = true;
            }
            element.near_end_armed = true;
        }

        if self.item_count < old_item_count {
            // Cache hygiene: a shrunk keying provably cannot produce an entry
            // recorded past the new end (inert on the uniform path).
            element.evict_measured_stale_indices();
        }

        // Measured anchor correction (variable extents only; the uniform path
        // measures nothing and never carries one): the previous layout recorded
        // how much taller or shorter the content above the viewport top actually
        // measured than the offset was planned against. Commit it here — the one
        // place the offset absorbs it, before this frame's window is planned —
        // unless a fling is live, in which case it keeps accumulating and lands
        // on the first rebuild after the fling stops. Placement has honored it
        // since the frame it was measured, so this moves nothing on screen; it
        // is still a `LAYOUT` report because the offset every row's origin is
        // derived from just changed. See the module docs' *Measured anchor
        // correction* section.
        if element.apply_pending_correction() {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let offset_before_clamp = element.offset;
        element.clamp_offset();
        if element.is_variable() && element.offset != offset_before_clamp {
            // Variable extents only: measurements can revise the content extent
            // out from under the offset, which moves every row — the same
            // extent-affecting rule the item-count/extent checks above follow.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let plan = element.desired_window();
        let delta = self.item_count as isize - old_item_count as isize;
        flags |= match self.key_of.as_ref() {
            Some(key_of) => {
                self.reconcile_keyed(prev, element, ctx, plan, key_of, delta, anchor_probe_missed)
            }
            None => {
                // A positional frame owns no key identities: drop whatever map a
                // previous keyed frame left (and, with it, every measurement
                // cached under one), so a list switched back to keyed later
                // cannot match against a window this path re-materialized by
                // index (a one-frame full re-materialization is the price of
                // swapping constructors mid-flight).
                element.reset_keyed_state();
                self.reconcile_positional(prev, element, ctx, plan)
            }
        };
        flags
    }

    fn teardown(&self, element: &mut ListViewWidget, ctx: &mut BuildCtx<'_>) {
        for (index, pod) in element.keys.iter().zip(element.children.iter_mut()) {
            crate::authoring::teardown_child(&(self.builder)(*index), pod, ctx);
        }
    }
}

impl Widget for ListViewWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vw = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let vh = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            // An unbounded height context: the list is as tall as its content.
            self.content_extent()
        };
        self.viewport = Size::new(vw, vh);
        self.clamp_offset();
        if let Some(estimate) = self.variable_estimate() {
            // Variable extents: rows size themselves and are measured here.
            self.layout_variable(ctx, vw, estimate);
        } else {
            // Uniform extent: every materialized row is exactly `item_extent` tall.
            let child_bc = BoxConstraints::tight(Size::new(vw, self.item_extent));
            for pod in &mut self.children {
                pod.layout_child(ctx, &child_bc);
            }
        }
        self.sync_child_origins();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.last_frame_time = ctx.frame_time();
        self.pump_fling(ctx);
        scene.push_clip(ctx.origin(), ctx.size());
        self.sync_child_origins();
        // The stretch is PAINT-ONLY, and load-bearingly so: no layout pass
        // (nor the windowing math) reads `edge_pull` or the intensity derived
        // from it, and none may start to. A layout-affecting animation must
        // request a relayout on every frame of its motion or the mobile
        // shell's intra-frame layout skip leaves it frozen
        // (`docs/WIDGETS_CODE_STANDARDS.md`'s animation-pacing rule) —
        // keeping the stretch out of every layout read is what makes that
        // irrelevant here, and is why there is no `request_layout` in this
        // path either: the settle/ballistic pump above already asks for every
        // frame the decaying stretch needs. Pushed INSIDE the viewport clip
        // so stretched rows can never paint past the viewport's edges.
        // Mirrors `ScrollWidget::paint`, over this widget's own `edge_pull`.
        let stretch = match self.effect {
            OverscrollEffect::Stretch => {
                stretch_about_edge(ctx.origin(), ctx.size(), self.edge_pull)
            }
            OverscrollEffect::Translate | OverscrollEffect::None => None,
        };
        if let Some(transform) = stretch {
            scene.push_transform(transform);
        }
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
        if stretch.is_some() {
            scene.pop_transform();
        }
        scene.pop_clip();
        // If the (now-known) viewport needs rows the materialized window does not
        // yet hold — first build, a constraint change, a fling that advanced the
        // offset past the buffer, or (variable extents) measurements that
        // revised the window out from under the plan rebuild worked from — ask
        // the shell for one more frame so the next rebuild re-windows. Converges
        // without idling under-materialized, and is the only path by which a
        // measurement changes what is materialized: layout and paint never build.
        // A correction this frame's layout recorded is committed by the *next*
        // rebuild, so a frame carrying one owes a continuation frame exactly
        // like an uncovered window does — the same one-frame convergence, and
        // the reason a measured correction never idles uncommitted. (While a
        // fling withholds it the pump above is already asking; the request here
        // is what covers the frame the fling stops on.)
        let WindowPlan { start, end, .. } = self.desired_window();
        let uncovered = self.item_count > 0 && !self.window_covers(start, end);
        if uncovered || self.pending_correction != 0.0 {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.event_time_ms();
        self.event_at(ctx, event, t)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A List container exposing its total item count and vertical scroll
        // range; the materialized rows contribute their own child nodes. Per the
        // pull-based seam, only the windowed rows are present — the accessible
        // set matches what is rendered, which is the documented v1 behavior.
        // `position_in_set` is not set on children: the semantics seam threads no
        // item index into `semantics_child`, and the generic builder need not
        // produce `ListItem`s, so the container's `size_of_set` carries the count.
        let max_offset = self.max_offset();
        let count = self.item_count;
        // The painted position, not the windowing offset: it is where the
        // content actually sits, and it is the value that stays put across a
        // correction commit (see the module docs' *Measured anchor
        // correction* section) — and, like `ScrollView`'s own semantics node,
        // it can momentarily read outside `[0, max_offset]` mid-overscroll
        // (see the module docs' *Overscroll and pull-to-refresh* section).
        // Placement, overscroll, and the raw offset all agree once the list is
        // in range, which is always true on the uniform path.
        let offset = self.painted_offset();
        ctx.push_container(
            Role::List,
            move |node| {
                node.set_size_of_set(count);
                node.set_scroll_y(offset);
                node.set_scroll_y_min(0.0);
                node.set_scroll_y_max(max_offset);
            },
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    crate::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::parity::{Bouncing, Clamping, DecelerationRate, NeverScrollable};
    use crate::physics::rubber_band::RubberBand;
    use crate::scroll::ScrollWidget;
    use frust_core::{RenderRoot, any};
    use std::any::Any;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    // --- A stateful row fixture: each built widget is stamped with a monotonic
    //     "generation" from a shared counter, so a relocated (state-preserving)
    //     row keeps its stamp while a rebuilt-fresh row gets a new one. ---

    struct GenView {
        gens: Rc<Cell<u64>>,
        seen: Rc<GenLog>,
        index: usize,
    }

    #[derive(Default)]
    struct GenLog {
        // index -> generation, last write wins (proves which pod rendered it).
        entries: std::cell::RefCell<std::collections::HashMap<usize, u64>>,
    }

    struct GenWidget {
        generation: u64,
        seen: Rc<GenLog>,
        index: usize,
    }

    impl View<()> for GenView {
        type Element = GenWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> GenWidget {
            let generation = self.gens.get();
            self.gens.set(generation + 1);
            GenWidget {
                generation,
                seen: self.seen.clone(),
                index: self.index,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut GenWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // A same-type in-place rebuild keeps the generation (state preserved).
            element.index = self.index;
            element.seen = self.seen.clone();
            ChangeFlags::NONE
        }
    }

    impl Widget for GenWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(bc.max())
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.seen
                .entries
                .borrow_mut()
                .insert(self.index, self.generation);
        }
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn list_widget(root: &RenderRoot<(), ListView<()>>) -> &ListViewWidget {
        let id = root.root_id().expect("root built");
        (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
            .downcast_ref::<ListViewWidget>()
            .expect("root is a ListViewWidget")
    }

    /// Drive a full rebuild → layout → paint frame at `ms`, returning the flags
    /// the rebuild reported (what the layout-skip contract is asserted against).
    fn frame(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        state: &mut (),
        window: Size,
        ms: f64,
    ) -> ChangeFlags {
        let flags = root.rebuild(logic, state);
        root.layout(window);
        let mut sink = NullScene;
        root.paint(&mut sink, FrameTime::from_nanos((ms * 1_000_000.0) as u64));
        flags
    }

    fn ev(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(10.0, y),
            button: PointerButton::Primary,
        })
    }

    // --- (1) Only the visible window materializes, at multiple offsets. ---

    #[test]
    fn only_the_visible_window_materializes() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);

        // First frame builds a conservative window; a second converges to the
        // real viewport (200 / 50 = 4 rows + 2*BUFFER).
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        let w = list_widget(&root);
        // window = floor(0/50)-2 .. ceil(200/50)+2 = 0 .. 6
        assert_eq!(w.window(), &[0, 1, 2, 3, 4, 5]);
        assert!(
            w.children.len() < 20,
            "only a handful of the 1000 rows are materialized"
        );
    }

    #[test]
    fn window_shifts_to_the_scrolled_offset() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Wheel to offset 500 (10 lines * 40px/line = 400... use a pixel scroll).
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 50.0),
                delta: ScrollDelta::Pixels(0.0, 500.0),
            },
        );
        frame(&mut root, &mut logic, &mut state, window, 32.0);

        let w = list_widget(&root);
        assert_eq!(w.offset(), 500.0);
        // window = floor(500/50)-2 .. ceil(700/50)+2 = 8 .. 16
        assert_eq!(w.window(), &[8, 9, 10, 11, 12, 13, 14, 15]);
    }

    // --- (2) A window shift relocates survivors (state preserved). ---

    #[test]
    fn window_shift_relocates_survivors_preserving_state() {
        let gens = Rc::new(Cell::new(0u64));
        let seen = Rc::new(GenLog::default());
        let gens_l = gens.clone();
        let seen_l = seen.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let gens = gens_l.clone();
            let seen = seen_l.clone();
            list_view(1000, 50.0, move |i| {
                any::<(), _>(GenView {
                    gens: gens.clone(),
                    seen: seen.clone(),
                    index: i,
                })
            })
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Record the generation stamped on a row that will survive the shift.
        let survivor_gen = seen.entries.borrow()[&3];

        // Nudge the offset down by one row (50px) so the window shifts by one but
        // index 3 stays inside it — via wheel so no child cancel is involved.
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, 50.0),
                delta: ScrollDelta::Pixels(0.0, 50.0),
            },
        );
        frame(&mut root, &mut logic, &mut state, window, 32.0);

        let entries = seen.entries.borrow();
        assert_eq!(
            entries[&3], survivor_gen,
            "a surviving row keeps its widget (and state): relocated, not rebuilt"
        );
        // window shifted to 0..7; index 6 newly entered → a strictly newer stamp.
        assert!(
            entries[&6] > survivor_gen,
            "a freshly-entered row is built anew (higher generation)"
        );
    }

    // --- (3) A fling advances the window across frames. ---

    #[test]
    fn fling_advances_the_window_across_frames() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Drag up past the slop, build velocity, release → a fling downward.
        root.event(&mut state, &ev(PointerPhase::Down, 180.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 120.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 48.0);
        root.event(&mut state, &ev(PointerPhase::Move, 60.0)); // build velocity
        root.event(&mut state, &ev(PointerPhase::Up, 60.0)); // release

        assert!(list_widget(&root).is_flinging(), "release starts a fling");
        let start_first = list_widget(&root).window()[0];

        // Pump several frames: paint advances the fling offset, the next rebuild
        // re-windows against it.
        for k in 0..8 {
            frame(
                &mut root,
                &mut logic,
                &mut state,
                window,
                64.0 + 16.0 * k as f64,
            );
        }

        assert!(
            list_widget(&root).window()[0] > start_first,
            "the fling carried the window to higher indices across frames"
        );
    }

    // --- (4) Extent / clamp math. ---

    #[test]
    fn extent_is_exact_and_offset_clamps_with_no_overscroll() {
        let mut w = ListViewWidget::new(100, 40.0);
        w.viewport = Size::new(200.0, 300.0);
        // extent = 100 * 40 = 4000; max_offset = 4000 - 300 = 3700.
        assert_eq!(w.max_offset(), 3700.0);
        w.set_offset(10_000.0);
        assert_eq!(w.offset(), 3700.0, "clamped to the end, no overscroll");
        w.set_offset(-50.0);
        assert_eq!(w.offset(), 0.0, "clamped to the top");

        // A viewport taller than the content pins the offset at zero.
        let mut short = ListViewWidget::new(2, 40.0);
        short.viewport = Size::new(200.0, 300.0);
        assert_eq!(short.max_offset(), 0.0);
    }

    #[test]
    fn empty_list_materializes_nothing() {
        fn logic(_: &mut ()) -> ListView<()> {
            list_view(0, 50.0, |i| any::<(), _>(gen_stub(i)))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);
        assert!(list_widget(&root).window().is_empty());
    }

    // A stateless stand-in row used by the window/fling/extent tests (no shared
    // generation bookkeeping needed there).
    pub(super) fn gen_stub(_index: usize) -> impl View<()> {
        crate::test_support::leaf(200.0, 50.0)
    }

    // --- (6) Semantics tree over the materialized window. ---

    /// A minimal `Role::ListItem`-reporting row, standing in for
    /// `material::list_item::ListItem` so this baseline module's own test
    /// suite carries no dependency on a design-system catalog (see the
    /// [module docs](self) — `ListView` itself takes no opinion on what a row
    /// is; any child view that contributes a `Role::ListItem` node exercises
    /// the same semantics-forwarding path).
    struct RoleListItemRow;

    struct RoleListItemRowWidget;

    impl View<()> for RoleListItemRow {
        type Element = RoleListItemRowWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RoleListItemRowWidget {
            RoleListItemRowWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut RoleListItemRowWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for RoleListItemRowWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(300.0, 56.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(frust_core::accesskit::Role::ListItem, |_node| {});
        }
    }

    #[test]
    fn semantics_is_a_list_container_over_the_windowed_rows() {
        use frust_core::accesskit::Role;
        use frust_text::TextContext;

        fn logic(_s: &mut ()) -> ListView<()> {
            list_view(1000, 56.0, |_i| any::<(), _>(RoleListItemRow))
        }
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(300.0, 200.0);

        // Converge the window (build → real-viewport rebuild), then collect.
        let mut tcx = TextContext::new();
        for _ in 0..2 {
            root.rebuild(&mut logic, &mut state);
            root.layout_with_text(window, &mut tcx as &mut dyn Any);
            let mut sink = NullScene;
            root.paint(&mut sink, FrameTime::ZERO);
        }

        let update = root.semantics();
        let materialized = list_widget(&root).window().len();

        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("the ListView contributes a Role::List container node");
        assert_eq!(
            list.size_of_set(),
            Some(1000),
            "the container advertises the full item count, not just the window"
        );
        assert_eq!(
            list.children().len(),
            materialized,
            "only the materialized rows are semantics children of the list"
        );

        // Every materialized row contributes its own Role::ListItem node.
        let list_items = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::ListItem)
            .count();
        assert_eq!(list_items, materialized);
    }

    // --- (7) Near-start "load older" callback. ---

    /// A state that counts near-start fires (the "load older" trigger).
    #[derive(Default)]
    struct Loads {
        count: u32,
    }

    /// Build a widget with a near-start callback installed, over a known viewport,
    /// bypassing the View layer (the callback path is exercised via `event_at`).
    fn near_start_widget(threshold: f64) -> ListViewWidget {
        let mut w = ListViewWidget::new(1000, 50.0);
        w.viewport = Size::new(200.0, 200.0);
        w.near_start_threshold = threshold;
        let cb: Rc<dyn Fn(&mut Loads)> = Rc::new(|s: &mut Loads| s.count += 1);
        w.on_near_start = Some(crate::authoring::erase_callback(&cb));
        w.near_start_armed = true;
        w
    }

    fn wheel(px: f64) -> InputEvent {
        InputEvent::Scroll {
            position: Point::new(10.0, 50.0),
            delta: ScrollDelta::Pixels(0.0, px),
        }
    }

    fn run_loads(w: &mut ListViewWidget, state: &mut Loads, e: &InputEvent) {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, e, 0.0);
    }

    #[test]
    fn on_near_start_fires_near_start_and_rearms_after_scrolling_away() {
        let mut w = near_start_widget(100.0);
        let mut state = Loads::default();

        // Scroll away from the start (past 2×threshold = 200) → no fire.
        run_loads(&mut w, &mut state, &wheel(500.0));
        assert_eq!(w.offset(), 500.0);
        assert_eq!(state.count, 0, "away from the start does not fire");

        // Scroll back within the threshold of the start → fires once.
        run_loads(&mut w, &mut state, &wheel(-460.0));
        assert_eq!(w.offset(), 40.0);
        assert_eq!(
            state.count, 1,
            "nearing the start fires the load-older hook"
        );

        // Staying near the start does not re-fire (edge-triggered, disarmed).
        run_loads(&mut w, &mut state, &wheel(-20.0));
        assert_eq!(w.offset(), 20.0);
        assert_eq!(state.count, 1, "no re-fire while still near the start");

        // Scroll away past 2×threshold to rearm, then back → fires again.
        run_loads(&mut w, &mut state, &wheel(500.0));
        assert_eq!(state.count, 1);
        run_loads(&mut w, &mut state, &wheel(-480.0));
        assert_eq!(w.offset(), 40.0);
        assert_eq!(state.count, 2, "rearmed after scrolling away, fires again");
    }

    #[test]
    fn on_near_start_does_not_fire_at_the_bottom() {
        let mut w = near_start_widget(100.0);
        let mut state = Loads::default();
        // Scroll to the very bottom — nowhere near the start edge.
        run_loads(&mut w, &mut state, &wheel(1_000_000.0));
        assert_eq!(w.offset(), w.max_offset());
        assert_eq!(state.count, 0, "the bottom is not the load-older edge");
    }

    #[test]
    fn near_start_content_growth_rearms_across_rebuild() {
        // A fling-free rebuild path: growing item_count (older rows loaded) rearms
        // the edge so a subsequent near-start approach fires again.
        let loads = Rc::new(Cell::new(0u32));
        let loads_l = loads.clone();
        let count = Rc::new(Cell::new(1000usize));
        let count_l = count.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let loads = loads_l.clone();
            list_view(count_l.get(), 50.0, |i| any::<(), _>(gen_stub(i)))
                .on_near_start(move |_: &mut ()| loads.set(loads.get() + 1), 100.0)
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // At the top (offset 0), a wheel nudge within the threshold fires once.
        root.event(&mut state, &wheel(10.0));
        assert_eq!(loads.get(), 1, "near-start fires at the top");
        // Still near start: no re-fire.
        root.event(&mut state, &wheel(5.0));
        assert_eq!(loads.get(), 1);

        // Grow the content (older rows loaded) → rebuild rearms the edge.
        count.set(2000);
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &wheel(5.0));
        assert_eq!(loads.get(), 2, "content growth rearms the load-older edge");
    }

    #[test]
    fn cancel_clears_a_pending_near_start_without_firing() {
        let mut w = near_start_widget(100.0);
        let mut state = Loads::default();
        // Simulate a fling-driven near-start recorded at paint time.
        w.pending_near_start = true;
        // A Cancel must drop it without invoking the callback.
        run_loads(&mut w, &mut state, &ev(PointerPhase::Cancel, 50.0));
        assert!(!w.pending_near_start);
        assert_eq!(state.count, 0, "Cancel never fires the callback");

        // A pending fire is otherwise delivered on the next (non-Cancel) event.
        w.pending_near_start = true;
        run_loads(&mut w, &mut state, &ev(PointerPhase::Down, 50.0));
        assert!(!w.pending_near_start);
        assert_eq!(
            state.count, 1,
            "a pending fire is delivered on the next event"
        );
    }

    // --- (8) Near-end "load newer" callback (mirrors near-start above). ---

    /// Build a widget with a near-end callback installed, over a known viewport,
    /// starting scrolled to the very end (mirrors [`near_start_widget`], which
    /// starts at the top).
    fn near_end_widget(threshold: f64) -> ListViewWidget {
        let mut w = ListViewWidget::new(1000, 50.0);
        w.viewport = Size::new(200.0, 200.0);
        w.near_end_threshold = threshold;
        let cb: Rc<dyn Fn(&mut Loads)> = Rc::new(|s: &mut Loads| s.count += 1);
        w.on_near_end = Some(crate::authoring::erase_callback(&cb));
        w.near_end_armed = true;
        w.offset = w.max_offset();
        w
    }

    #[test]
    fn on_near_end_fires_near_end_and_rearms_after_scrolling_away() {
        let mut w = near_end_widget(100.0);
        let mut state = Loads::default();
        let start_offset = w.offset();

        // Scroll away from the end (past 2×threshold = 200) → no fire.
        run_loads(&mut w, &mut state, &wheel(-500.0));
        assert_eq!(w.offset(), start_offset - 500.0);
        assert_eq!(state.count, 0, "away from the end does not fire");

        // Scroll back within the threshold of the end → fires once.
        run_loads(&mut w, &mut state, &wheel(460.0));
        assert_eq!(w.offset(), start_offset - 40.0);
        assert_eq!(state.count, 1, "nearing the end fires the load-newer hook");

        // Staying near the end does not re-fire (edge-triggered, disarmed).
        run_loads(&mut w, &mut state, &wheel(20.0));
        assert_eq!(w.offset(), start_offset - 20.0);
        assert_eq!(state.count, 1, "no re-fire while still near the end");

        // Scroll away past 2×threshold to rearm, then back → fires again.
        run_loads(&mut w, &mut state, &wheel(-500.0));
        assert_eq!(state.count, 1);
        run_loads(&mut w, &mut state, &wheel(480.0));
        assert_eq!(w.offset(), start_offset - 40.0);
        assert_eq!(state.count, 2, "rearmed after scrolling away, fires again");
    }

    #[test]
    fn on_near_end_does_not_fire_at_the_top() {
        let mut w = near_end_widget(100.0);
        let mut state = Loads::default();
        // Scroll all the way to the very top — nowhere near the end edge.
        run_loads(&mut w, &mut state, &wheel(-1_000_000.0));
        assert_eq!(w.offset(), 0.0);
        assert_eq!(state.count, 0, "the top is not the load-newer edge");
    }

    #[test]
    fn near_end_content_growth_rearms_across_rebuild() {
        // Growing item_count (newer rows appended) rearms the edge so a
        // subsequent near-end approach fires again.
        let loads = Rc::new(Cell::new(0u32));
        let loads_l = loads.clone();
        let count = Rc::new(Cell::new(1000usize));
        let count_l = count.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let loads = loads_l.clone();
            list_view(count_l.get(), 50.0, |i| any::<(), _>(gen_stub(i)))
                .on_near_end(move |_: &mut ()| loads.set(loads.get() + 1), 100.0)
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Scroll to the very bottom, within the threshold of the end → fires
        // once.
        root.event(&mut state, &wheel(1_000_000.0));
        assert_eq!(loads.get(), 1, "near-end fires at the bottom");
        // Still near end: no re-fire.
        root.event(&mut state, &wheel(5.0));
        assert_eq!(loads.get(), 1);

        // Grow the content (newer rows appended) → rebuild rearms the edge, and
        // the new (farther) bottom is no longer within the threshold.
        count.set(2000);
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &wheel(1_000_000.0));
        assert_eq!(loads.get(), 2, "content growth rearms the load-newer edge");
    }

    #[test]
    fn cancel_clears_a_pending_near_end_without_firing() {
        let mut w = near_end_widget(100.0);
        let mut state = Loads::default();
        // Simulate a fling-driven near-end recorded at paint time.
        w.pending_near_end = true;
        // A Cancel must drop it without invoking the callback.
        run_loads(&mut w, &mut state, &ev(PointerPhase::Cancel, 50.0));
        assert!(!w.pending_near_end);
        assert_eq!(state.count, 0, "Cancel never fires the callback");

        // A pending fire is otherwise delivered on the next (non-Cancel) event.
        w.pending_near_end = true;
        run_loads(&mut w, &mut state, &ev(PointerPhase::Down, 50.0));
        assert!(!w.pending_near_end);
        assert_eq!(
            state.count, 1,
            "a pending fire is delivered on the next event"
        );
    }

    // --- (8b) Overscroll and pull-to-refresh: `ListViewWidget::offset` (the
    //      windowing offset) must never leave `[0, max_offset]`, only
    //      `ListViewWidget::overscroll` may — mirrors `scroll.rs`'s own
    //      overscroll/refresh test group, see the module docs' *Overscroll
    //      and pull-to-refresh* section. ---

    /// A stateless stand-in row, like [`gen_stub`], used by every test in this
    /// group (materialization-bounds assertions only, no per-row identity).
    fn overscroll_logic(item_count: usize) -> impl FnMut(&mut ()) -> ListView<()> {
        move |_: &mut ()| list_view(item_count, 50.0, |i| any::<(), _>(gen_stub(i)))
    }

    /// [`overscroll_logic`] with the pre-seam [`RubberBand`] feel pinned
    /// explicitly — the fixture every *rubber-band* pin in this file builds
    /// from now that the widget's own default is the platform-adaptive physics
    /// (bouncing here, clamping on Android), each with curves of its own. A
    /// test whose assertions are a `0.5`-resisted displacement or a
    /// `SETTLE_DECAY` trace is pinning *this* physics, not the default; the
    /// `ScrollView` twin of this fixture is `scroll.rs`'s
    /// `laid_out_rubber_band`.
    fn rubber_band_logic(item_count: usize) -> impl FnMut(&mut ()) -> ListView<()> {
        move |_: &mut ()| {
            list_view(item_count, 50.0, |i| any::<(), _>(gen_stub(i))).physics(RubberBand::new())
        }
    }

    #[test]
    fn top_overscroll_resists_never_moves_the_windowing_offset_and_settles_with_no_refresh() {
        let refreshes = Rc::new(Cell::new(0u32));
        let refreshes_l = refreshes.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let refreshes = refreshes_l.clone();
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .physics(RubberBand::new())
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Down, then a drag downward crossing the slop takes the gesture over.
        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // 40px > slop → takeover
        assert!(list_widget(&root).scrolling);
        assert_eq!(
            list_widget(&root).offset(),
            0.0,
            "the takeover move does not itself scroll"
        );
        frame(&mut root, &mut logic, &mut state, window, 48.0);

        // Drag 20px further down past the already-at-top edge → resisted
        // overscroll, but the windowing offset stays put and no row
        // materializes before index 0.
        root.event(&mut state, &ev(PointerPhase::Move, 110.0));
        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            0.0,
            "the windowing offset never leaves [0, max]"
        );
        assert_eq!(
            w.overscroll, -10.0,
            "overscroll is the raw excess (-20) * OVERSCROLL_RESISTANCE (0.5), \
             matching ScrollView's own resistance exactly"
        );
        assert_eq!(w.window()[0], 0, "no row materializes before index 0");

        // Release under the trigger (|-10| < 64) → settles, never refreshes.
        root.event(&mut state, &ev(PointerPhase::Up, 110.0));
        let w = list_widget(&root);
        assert!(w.settling, "an overscrolled release settles, never flings");
        assert!(!w.is_flinging());
        assert_eq!(
            refreshes.get(),
            0,
            "release under the trigger does not refresh"
        );

        // Pump frames until the settle completes.
        let mut ms = 64.0;
        for _ in 0..30 {
            frame(&mut root, &mut logic, &mut state, window, ms);
            ms += 16.0;
            if !list_widget(&root).settling {
                break;
            }
        }
        let w = list_widget(&root);
        assert!(!w.settling, "the settle terminated");
        assert_eq!(
            w.overscroll, 0.0,
            "the surface settles back to the clamped edge"
        );
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn on_refresh_release_fires_once_past_the_trigger_and_only_on_release() {
        let refreshes = Rc::new(Cell::new(0u32));
        let refreshes_l = refreshes.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let refreshes = refreshes_l.clone();
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .physics(RubberBand::new())
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 290.0)); // raw -200 → -100

        let w = list_widget(&root);
        assert_eq!(w.overscroll, -100.0, "past the trigger (|-100| > 64)");
        assert_eq!(
            w.offset(),
            0.0,
            "windowing offset stays 0 even far past the trigger"
        );
        assert_eq!(w.window()[0], 0, "no row materializes before index 0");
        assert_eq!(refreshes.get(), 0, "no fire before release");

        root.event(&mut state, &ev(PointerPhase::Up, 290.0));
        assert_eq!(refreshes.get(), 1, "release past the trigger fires once");
        assert!(list_widget(&root).settling);
    }

    #[test]
    fn bottom_overscroll_displaces_and_settles_but_never_fires_refresh() {
        let refreshes = Rc::new(Cell::new(0u32));
        let refreshes_l = refreshes.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let refreshes = refreshes_l.clone();
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .physics(RubberBand::new())
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Scroll to the very bottom first.
        root.event(&mut state, &wheel(1_000_000.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        let max_offset = list_widget(&root).max_offset();
        assert_eq!(list_widget(&root).offset(), max_offset);

        // Down, then a drag *upward* crossing the slop takes the gesture over
        // (dragging the finger up exposes content past the bottom edge).
        root.event(&mut state, &ev(PointerPhase::Down, 200.0));
        frame(&mut root, &mut logic, &mut state, window, 48.0);
        root.event(&mut state, &ev(PointerPhase::Move, 160.0)); // 40px > slop → takeover
        assert_eq!(
            list_widget(&root).offset(),
            max_offset,
            "the takeover move does not itself scroll"
        );
        frame(&mut root, &mut logic, &mut state, window, 64.0);

        // Drag 60px further up past the bottom edge → resisted overscroll,
        // the windowing offset pinned at max_offset, no row past the end.
        root.event(&mut state, &ev(PointerPhase::Move, 100.0));
        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            max_offset,
            "the windowing offset stays pinned at max_offset"
        );
        assert_eq!(
            w.overscroll, 30.0,
            "overscroll is the raw excess (60) * OVERSCROLL_RESISTANCE (0.5)"
        );
        assert_eq!(
            *w.window().last().unwrap(),
            999,
            "no row materializes past the last item"
        );

        root.event(&mut state, &ev(PointerPhase::Up, 100.0));
        let w = list_widget(&root);
        assert!(
            w.settling,
            "a bottom overscroll release settles, never flings"
        );
        assert_eq!(
            refreshes.get(),
            0,
            "a bottom overscroll never fires refresh"
        );

        let mut ms = 80.0;
        for _ in 0..30 {
            frame(&mut root, &mut logic, &mut state, window, ms);
            ms += 16.0;
            if !list_widget(&root).settling {
                break;
            }
        }
        let w = list_widget(&root);
        assert!(!w.settling);
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.offset(), max_offset);
        assert_eq!(refreshes.get(), 0, "still never fired");
    }

    #[test]
    fn cancel_during_overscroll_never_fires_refresh_and_snaps_back() {
        let refreshes = Rc::new(Cell::new(0u32));
        let refreshes_l = refreshes.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let refreshes = refreshes_l.clone();
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .physics(RubberBand::new())
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 290.0)); // past trigger
        assert_eq!(list_widget(&root).overscroll, -100.0);

        // A Cancel (gesture steal) must not fire on_refresh_release and snaps
        // the overscroll away with no settle animation.
        root.event(&mut state, &ev(PointerPhase::Cancel, 290.0));
        let w = list_widget(&root);
        assert_eq!(
            refreshes.get(),
            0,
            "Cancel never fires the refresh callback"
        );
        assert_eq!(
            w.overscroll, 0.0,
            "Cancel snaps the surface back into range"
        );
        assert!(!w.settling);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn wheel_never_overscrolls_past_either_edge_and_starts_no_settle() {
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        let mut logic = overscroll_logic(1000);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // A large negative wheel delta at the top stays hard-clamped at 0.
        root.event(&mut state, &wheel(-5000.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 0.0);
        assert_eq!(w.overscroll, 0.0, "wheel input never overscrolls the top");
        assert!(!w.settling, "wheel input starts no settle animation");

        // A huge positive wheel delta at the bottom stays hard-clamped too.
        root.event(&mut state, &wheel(1_000_000.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), w.max_offset());
        assert_eq!(
            w.overscroll, 0.0,
            "wheel input never overscrolls the bottom"
        );
        assert!(!w.settling);
    }

    #[test]
    fn near_start_and_refresh_fire_from_one_continuous_drag_sequence() {
        // Criterion 4: `on_near_start` fires on approach, then continuing the
        // same gesture past the top into overscroll and releasing fires
        // `on_refresh_release` too — two independent signals, one gesture.
        let loads = Rc::new(Cell::new(0u32));
        let refreshes = Rc::new(Cell::new(0u32));
        let (loads_l, refreshes_l) = (loads.clone(), refreshes.clone());
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let (loads, refreshes) = (loads_l.clone(), refreshes_l.clone());
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .on_near_start(move |_: &mut ()| loads.set(loads.get() + 1), 100.0)
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // Start scrolled away from the top (150 > threshold, so approaching it
        // is a real edge crossing, not a trivial already-there state).
        root.event(&mut state, &wheel(150.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        assert_eq!(list_widget(&root).offset(), 150.0);
        assert_eq!(loads.get(), 0);

        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // 40px > slop → takeover
        frame(&mut root, &mut logic, &mut state, window, 48.0);

        // Drag down 80px: offset 150 -> 70, inside the near-start threshold →
        // fires once, in range (no overscroll yet).
        root.event(&mut state, &ev(PointerPhase::Move, 170.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 70.0);
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(loads.get(), 1, "near-start fires on approach");
        assert_eq!(refreshes.get(), 0, "still in range, no refresh yet");

        // Keep dragging the same gesture on down past the top, deep into
        // overscroll past the refresh trigger.
        root.event(&mut state, &ev(PointerPhase::Move, 500.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 0.0, "windowing offset stays clamped");
        assert!(
            w.overscroll < -64.0,
            "well past the refresh trigger (REFRESH_TRIGGER_PX)"
        );
        assert_eq!(loads.get(), 1, "near-start does not re-fire mid-overscroll");

        root.event(&mut state, &ev(PointerPhase::Up, 500.0));
        assert_eq!(
            refreshes.get(),
            1,
            "refresh fires on release, from the same gesture"
        );
        assert_eq!(loads.get(), 1, "and near-start's single fire still stands");
    }

    // --- (8c) A converging `max_offset` during content growth (rule 5:
    //      overscroll resistance always reads the *current* max_offset, never
    //      one cached at takeover — the mechanism the *Variable extents*
    //      docs section relies on; exercised here at the uniform level, since
    //      `apply_drag_offset` reads `max_offset()` fresh regardless of
    //      mode). ---

    #[test]
    fn overscroll_resistance_tracks_a_max_offset_that_grows_mid_drag() {
        // A short list (10 rows * 50px = 500 content over a 200px viewport,
        // max_offset = 300) scrolled to the bottom, then overscrolled past it.
        let mut logic = rubber_band_logic(10);
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        root.event(&mut state, &wheel(1_000_000.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        assert_eq!(list_widget(&root).max_offset(), 300.0);
        assert_eq!(list_widget(&root).offset(), 300.0);

        root.event(&mut state, &ev(PointerPhase::Down, 200.0));
        root.event(&mut state, &ev(PointerPhase::Move, 160.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 48.0);

        // Drag 60px further up past the (still 300px) bottom edge.
        root.event(&mut state, &ev(PointerPhase::Move, 100.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 300.0);
        assert_eq!(
            w.overscroll, 30.0,
            "resisted against the old max_offset (300)"
        );

        // Grow the content mid-drag (still `w.scrolling`): 20 rows now, content
        // 1000px, max_offset 800 — a rebuild interleaved into the same drag.
        logic = rubber_band_logic(20);
        frame(&mut root, &mut logic, &mut state, window, 64.0);
        assert_eq!(list_widget(&root).max_offset(), 800.0);

        // The *same* continuing drag now reads the new, larger max_offset: 20
        // more px up lands the drag position back in range. The surface was
        // *showing* content y 330 (offset 300 + the 30px it was displaced by),
        // so the grown extent makes that 330 a real in-range offset and the
        // next 20px of finger takes it to 350 — the drag position is a
        // physics-mapped accumulator, so what the resistance already swallowed
        // is not handed back when the content grows under the finger.
        root.event(&mut state, &ev(PointerPhase::Move, 80.0));
        let w = list_widget(&root);
        assert_eq!(
            w.overscroll, 0.0,
            "the grown content absorbed what used to be overscroll"
        );
        assert_eq!(
            w.offset(),
            350.0,
            "the windowing offset advanced by exactly the finger delta, now in range"
        );
    }

    // --- (8c) The physics seam: the default `RubberBand` install produces the
    //      same numbers this widget has always produced, and `edge_pull`
    //      tracks the displacement exactly while nothing is rejected (the
    //      `ScrollView` twins of these two live in `scroll.rs`). ---

    #[test]
    fn rubber_band_drag_mapping_matches_legacy_math() {
        let mut logic = rubber_band_logic(1000);
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // In range: the drag delta passes through the physics untouched, into
        // the windowing offset, with no displacement at all.
        root.event(&mut state, &ev(PointerPhase::Down, 200.0));
        root.event(&mut state, &ev(PointerPhase::Move, 160.0)); // takeover
        root.event(&mut state, &ev(PointerPhase::Move, 100.0)); // 60px up
        let w = list_widget(&root);
        assert_eq!(w.offset(), 60.0, "an in-range drag maps one-for-one");
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.edge_pull, 0.0);
        root.event(&mut state, &ev(PointerPhase::Cancel, 100.0));

        // Past the top: half the raw excess shows, the windowing offset pinned.
        root.event(&mut state, &wheel(-1_000_000.0)); // back to the top edge
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        root.event(&mut state, &ev(PointerPhase::Move, 110.0)); // 20px past the top
        let w = list_widget(&root);
        assert_eq!(w.offset(), 0.0, "the windowing offset never leaves range");
        assert_eq!(w.overscroll, -10.0, "raw excess (-20) halved by resistance");
        root.event(&mut state, &ev(PointerPhase::Cancel, 110.0));

        // Past the bottom: the same rule against a fresh `max_offset`.
        root.event(&mut state, &wheel(1_000_000.0));
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        let max_offset = list_widget(&root).max_offset();
        root.event(&mut state, &ev(PointerPhase::Down, 200.0));
        root.event(&mut state, &ev(PointerPhase::Move, 160.0)); // takeover
        root.event(&mut state, &ev(PointerPhase::Move, 100.0)); // 60px past the bottom
        let w = list_widget(&root);
        assert_eq!(w.offset(), max_offset);
        assert_eq!(w.overscroll, 30.0, "raw excess (60) halved by resistance");
    }

    #[test]
    fn edge_pull_equals_overscroll_under_rubber_band() {
        let mut logic = rubber_band_logic(1000);
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        root.event(&mut state, &ev(PointerPhase::Move, 110.0)); // 20px past the top
        let w = list_widget(&root);
        assert_eq!(w.overscroll, -10.0);
        assert_eq!(
            w.edge_pull, -10.0,
            "nothing rejected → the pull is the displacement, same sign"
        );

        // Release, then settle: both decay together and both reach zero.
        root.event(&mut state, &ev(PointerPhase::Up, 110.0));
        assert!(list_widget(&root).settling);
        let mut ms = 32.0;
        let mut eased = false;
        for _ in 0..40 {
            frame(&mut root, &mut logic, &mut state, window, ms);
            ms += 16.0;
            let w = list_widget(&root);
            assert_eq!(
                w.edge_pull, w.overscroll,
                "the pull tracks the displacement through the whole settle"
            );
            if w.overscroll < 0.0 && w.overscroll > -10.0 {
                eased = true;
            }
            if !w.settling {
                break;
            }
        }
        assert!(eased, "the settle eased through intermediate values");
        let w = list_widget(&root);
        assert!(!w.settling, "the settle terminates");
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.edge_pull, 0.0, "a completed settle leaves no pull");
    }

    // --- The platform default (`physics::default_physics`): bouncing on this
    //      host, clamping on Android. The `ScrollView` twins of this group
    //      live in `scroll.rs`; these pin that the windowing/displacement
    //      split reaches the same numbers through this widget's own drag
    //      path. ---

    fn assert_close(actual: f64, expected: f64, epsilon: f64, what: &str) {
        assert!(
            (actual - expected).abs() < epsilon,
            "{what}: {actual} is not within {epsilon} of {expected}"
        );
    }

    #[test]
    fn a_fresh_list_installs_the_platform_default() {
        let mut logic = overscroll_logic(1000);
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        frame(&mut root, &mut logic, &mut (), Size::new(200.0, 200.0), 0.0);
        let w = list_widget(&root);
        assert_eq!(
            format!("{:?}", w.physics),
            format!("{:?}", crate::physics::default_physics())
        );
        assert_eq!(w.effect, crate::physics::default_overscroll_effect());
    }

    #[test]
    fn default_drag_tension_tightens_with_depth() {
        let mut logic = overscroll_logic(1000);
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // 40px > slop → takeover
        frame(&mut root, &mut logic, &mut state, window, 32.0);

        // 20px past the top from zero depth: 20 · 0.52 = 10.4 displaced, and
        // the windowing offset still pinned at 0.
        root.event(&mut state, &ev(PointerPhase::Move, 110.0));
        let first = list_widget(&root).overscroll;
        assert_eq!(list_widget(&root).offset(), 0.0);
        assert_close(
            first,
            -20.0 * DecelerationRate::NORMAL_FRICTION,
            1e-12,
            "the first past-edge move",
        );

        // 20px more, now 10.4px deep in a 200px viewport: the factor tightens
        // to 0.52·(1 − 0.052)² = 0.46732608, adding 9.3465216 for a total of
        // 19.7465216.
        root.event(&mut state, &ev(PointerPhase::Move, 130.0));
        let w = list_widget(&root);
        assert_close(w.overscroll, -19.746_521_6, 1e-9, "the accumulated pull");
        assert!(
            (w.overscroll - first).abs() < first.abs(),
            "the deeper pull displaces less per raw px"
        );
        assert_eq!(w.offset(), 0.0, "…and the windowing offset never moves");
        assert_eq!(w.window()[0], 0, "no row materializes before index 0");
    }

    /// The signed start velocity of whatever post-release motion the last `Up`
    /// produced — the physics-supplied curve's own, the legacy fling's, or
    /// `0.0` for a release that started no motion at all. The `ScrollView`
    /// twin of this helper reads the same two fields.
    fn release_velocity(w: &ListViewWidget) -> f64 {
        match w.ballistic.as_ref() {
            Some(state) => state.sim.dx(0.0),
            None => w.fling.unwrap_or(0.0),
        }
    }

    /// The `ScrollView` twin of this test lives in `scroll.rs`, beside
    /// `default_carried_momentum_compounds_a_refling` — the same-direction pin
    /// this one is the mirror image of.
    #[test]
    fn reverse_refling_keeps_the_fingers_velocity() {
        // A bare list (no rows materialized — this pins release velocity, not
        // windowing), parked mid-content so neither edge is in play.
        let mut w = ListViewWidget::new(1000, 50.0);
        w.viewport = Size::new(200.0, 200.0);
        dispatch_list(&mut w, &wheel(1000.0), 0.0);
        assert_eq!(w.offset(), 1000.0, "the fixture parked mid-content");

        // A downward fling: 50px of finger travel up over 32ms.
        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch_list(&mut w, &ev(PointerPhase::Move, 50.0), 32.0);
        dispatch_list(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert_close(release_velocity(&w), 1562.5, 1e-9, "the first release");

        // The finger lands on that live curve and flicks back the other way,
        // just as fast. The interrupted motion's momentum must not be added to
        // a release pointing the other way — it would cancel the flick out (or
        // reverse it), and the list would ignore the finger entirely.
        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 125.0), 64.0); // takeover
        dispatch_list(&mut w, &ev(PointerPhase::Move, 150.0), 80.0);
        dispatch_list(&mut w, &ev(PointerPhase::Up, 150.0), 80.0);
        assert_close(
            release_velocity(&w),
            -1562.5,
            1e-9,
            "the reverse re-fling runs at the finger's own velocity",
        );
        assert!(
            w.ballistic.is_some(),
            "…as a real ballistic curve, not a stalled remnant"
        );
    }

    /// The `ScrollView` twins of this pair live in `scroll.rs`
    /// (`momentum_retain_threshold_refuses_a_weak_refling`): the retain gate
    /// compares a release against the physics' **mapped** share of the
    /// interrupted velocity, not the raw interrupted speed. Interrupted at
    /// 1000 px/s, `Bouncing::new().carried_momentum(1000.0)` maps to ~649.7,
    /// putting the retain threshold at ~324.8 — well under the raw-carried
    /// threshold (500) the pre-fix gate used.
    #[test]
    fn momentum_retain_threshold_refuses_a_weak_refling() {
        let mut w = ListViewWidget::new(1000, 50.0);
        w.viewport = Size::new(200.0, 200.0);
        dispatch_list(&mut w, &wheel(1000.0), 0.0);
        assert_eq!(w.offset(), 1000.0, "the fixture parked mid-content");

        // Interrupted motion at 1000 px/s.
        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 78.0), 16.0); // takeover
        dispatch_list(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch_list(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        assert_close(release_velocity(&w), 1000.0, 1e-9, "the interrupted motion");

        let mapped = Bouncing::new().carried_momentum(1000.0);
        let threshold = MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped;
        assert!(
            (300.0..350.0).contains(&threshold),
            "the fixture's release values must straddle the threshold: {threshold}"
        );

        // Same-direction re-flick at 300 px/s — under the mapped threshold.
        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 80.0), 64.0); // takeover
        dispatch_list(&mut w, &ev(PointerPhase::Move, 70.0), 148.0); // 30px / 100ms → 300 px/s
        dispatch_list(&mut w, &ev(PointerPhase::Up, 70.0), 148.0);
        assert_close(
            release_velocity(&w),
            300.0,
            1e-9,
            "a release under the mapped threshold carries nothing forward",
        );
    }

    /// The strong-side twin of `momentum_retain_threshold_refuses_a_weak_refling`:
    /// a release over the same mapped threshold carries `mapped` forward
    /// exactly, pre-clamp.
    #[test]
    fn momentum_retain_threshold_carries_a_strong_refling() {
        let mut w = ListViewWidget::new(1000, 50.0);
        w.viewport = Size::new(200.0, 200.0);
        dispatch_list(&mut w, &wheel(1000.0), 0.0);
        assert_eq!(w.offset(), 1000.0, "the fixture parked mid-content");

        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 78.0), 16.0); // takeover
        dispatch_list(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch_list(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        assert_close(release_velocity(&w), 1000.0, 1e-9, "the interrupted motion");

        let mapped = Bouncing::new().carried_momentum(1000.0);
        let threshold = MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped;
        assert!(
            (300.0..350.0).contains(&threshold),
            "the fixture's release values must straddle the threshold: {threshold}"
        );

        // Same-direction re-flick at 350 px/s — over the mapped threshold.
        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 48.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 80.0), 64.0); // takeover
        dispatch_list(&mut w, &ev(PointerPhase::Move, 65.0), 148.0); // 35px / 100ms → 350 px/s
        dispatch_list(&mut w, &ev(PointerPhase::Up, 65.0), 148.0);
        assert_close(
            release_velocity(&w),
            350.0 + mapped,
            1e-9,
            "a release over the mapped threshold carries `mapped` forward exactly",
        );
    }

    // --- Pull-to-refresh under both shipped defaults — the twins of
    //      `scroll.rs`'s pair, over this widget's own `edge_pull`. ---

    #[test]
    fn refresh_trigger_under_the_bouncing_default() {
        let refreshes = Rc::new(Cell::new(0u32));
        let refreshes_l = refreshes.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let refreshes = refreshes_l.clone();
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        // 110px of raw pull at zero depth maps to 57.2 — under the trigger.
        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 200.0));
        let w = list_widget(&root);
        assert_close(w.edge_pull, -57.2, 1e-9, "under the trigger");
        assert_eq!(
            w.edge_pull, w.overscroll,
            "a bouncing surface rejects nothing, so the pull IS the displacement"
        );
        root.event(&mut state, &ev(PointerPhase::Up, 200.0));
        assert_eq!(refreshes.get(), 0, "release under the trigger never fires");

        // 150px of raw pull maps to 78.0 — past it.
        root.event(&mut state, &ev(PointerPhase::Cancel, 200.0));
        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 48.0);
        root.event(&mut state, &ev(PointerPhase::Move, 240.0));
        let w = list_widget(&root);
        assert_close(w.edge_pull, -78.0, 1e-9, "past the trigger");
        assert!(crossed_refresh_trigger(w.edge_pull));
        assert_eq!(refreshes.get(), 0, "no fire before release");
        root.event(&mut state, &ev(PointerPhase::Up, 240.0));
        assert_eq!(refreshes.get(), 1, "release past the trigger fires once");
    }

    #[test]
    fn refresh_trigger_under_a_clamping_physics() {
        // Android's default, simulated on the host.
        let refreshes = Rc::new(Cell::new(0u32));
        let refreshes_l = refreshes.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let refreshes = refreshes_l.clone();
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i)))
                .physics(Clamping::new())
                .on_refresh_release(move |_: &mut ()| refreshes.set(refreshes.get() + 1))
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        let window = Size::new(200.0, 200.0);
        frame(&mut root, &mut logic, &mut state, window, 0.0);
        frame(&mut root, &mut logic, &mut state, window, 16.0);

        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 32.0);
        root.event(&mut state, &ev(PointerPhase::Move, 140.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 0.0, "a clamping surface never displaces");
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.edge_pull, -50.0, "…but reports the whole rejected pull");
        root.event(&mut state, &ev(PointerPhase::Up, 140.0));
        assert_eq!(refreshes.get(), 0, "50px of raw pull is under the trigger");

        root.event(&mut state, &ev(PointerPhase::Cancel, 140.0));
        root.event(&mut state, &ev(PointerPhase::Down, 50.0));
        root.event(&mut state, &ev(PointerPhase::Move, 90.0)); // takeover
        frame(&mut root, &mut logic, &mut state, window, 48.0);
        root.event(&mut state, &ev(PointerPhase::Move, 190.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 0.0);
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.edge_pull, -100.0, "100px of raw pull, none of it shown");
        assert_eq!(w.window()[0], 0, "no row materializes before index 0");
        root.event(&mut state, &ev(PointerPhase::Up, 190.0));
        assert_eq!(refreshes.get(), 1, "past 64px of raw pull, it fires once");
        assert!(
            list_widget(&root).settling,
            "the rejected pull settles rather than springs — nothing displaced"
        );

        let mut ms = 64.0;
        for _ in 0..40 {
            frame(&mut root, &mut logic, &mut state, window, ms);
            ms += 16.0;
            if !list_widget(&root).settling {
                break;
            }
        }
        let w = list_widget(&root);
        assert!(!w.settling, "the settle terminated");
        assert_eq!(w.edge_pull, 0.0, "…leaving no pull for a stretch to paint");
    }

    // --- (07) The public builder surface: `.physics(...)` — the ListView twin
    //      of `scroll.rs`'s own group of the same name. ---

    #[test]
    fn physics_builder_installs_custom_physics() {
        let window = Size::new(200.0, 200.0);

        // A ListView built with `.physics(NeverScrollable::new())`: a drag
        // past slop does not scroll.
        let mut never_logic = move |_: &mut ()| -> ListView<()> {
            list_view(1000, 50.0, |i| any::<(), _>(gen_stub(i))).physics(NeverScrollable::new())
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        frame(&mut root, &mut never_logic, &mut state, window, 0.0);
        frame(&mut root, &mut never_logic, &mut state, window, 16.0);
        root.event(&mut state, &ev(PointerPhase::Down, 200.0));
        root.event(&mut state, &ev(PointerPhase::Move, 160.0)); // would cross slop
        root.event(&mut state, &ev(PointerPhase::Move, 100.0));
        let w = list_widget(&root);
        assert_eq!(w.offset(), 0.0, "NeverScrollable must refuse the drag");
        assert!(
            !w.scrolling,
            "NeverScrollable must never take the gesture over"
        );

        // A default-built twin (no `.physics(...)` call) still scrolls
        // normally under `RubberBand` — same numbers as
        // `rubber_band_drag_mapping_matches_legacy_math`.
        let mut default_logic = overscroll_logic(1000);
        let mut default_root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut default_state = ();
        frame(
            &mut default_root,
            &mut default_logic,
            &mut default_state,
            window,
            0.0,
        );
        frame(
            &mut default_root,
            &mut default_logic,
            &mut default_state,
            window,
            16.0,
        );
        default_root.event(&mut default_state, &ev(PointerPhase::Down, 200.0));
        default_root.event(&mut default_state, &ev(PointerPhase::Move, 160.0));
        default_root.event(&mut default_state, &ev(PointerPhase::Move, 100.0));
        assert_eq!(
            list_widget(&default_root).offset(),
            60.0,
            "the default twin scrolls normally"
        );
    }

    // --- (8d) Variable-extent overscroll: the same bounds hold when rows
    //      size themselves (see the *Variable extents* module docs section
    //      for `estimated_item_extent`). ---

    #[test]
    fn variable_extent_top_overscroll_never_materializes_before_index_zero() {
        let fx = VarRows::new(tall_ids(60));
        let mut logic = fx.logic_rubber_band();
        let mut root = converged_var(&mut logic, &fx);
        assert_eq!(list_widget(&root).window()[0], 0);

        root.event(&mut (), &ev(PointerPhase::Down, 50.0));
        root.event(&mut (), &ev(PointerPhase::Move, 90.0)); // 40px > slop → takeover
        var_frame(&mut root, &mut logic, &fx, 100.0);

        // Drag past the top edge.
        root.event(&mut (), &ev(PointerPhase::Move, 150.0));
        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            0.0,
            "the windowing offset never leaves [0, max] in variable-extent mode either"
        );
        assert!(w.overscroll < 0.0, "a past-top drag overscrolls");
        assert_eq!(w.window()[0], 0, "no row materializes before index 0");

        root.event(&mut (), &ev(PointerPhase::Up, 150.0));
        assert!(
            list_widget(&root).settling,
            "an overscrolled release settles in variable-extent mode too"
        );

        // Pump frames until the settle completes.
        for n in 0..60 {
            var_frame(&mut root, &mut logic, &fx, 116.0 + 16.0 * n as f64);
            if !list_widget(&root).settling {
                break;
            }
        }
        let w = list_widget(&root);
        assert!(!w.settling);
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.offset(), 0.0);
    }

    // --- (8e) Nested scrolling: innermost-wins arbitration with this widget as
    //     the OUTER surface, over a row that owns a scroll of its own. The
    //     mirror case (a nested `ListView` under a `ScrollView`) lives beside
    //     the seam itself, in `scroll.rs`. See the module docs' *Nested
    //     scrolling*. ---

    /// A bare list for the nested-scroll fixtures: `count` rows of `extent` px
    /// in a `viewport_h`-tall viewport, no rows materialized yet ([`wire_row`]
    /// installs the one that matters).
    fn nested_list(count: usize, extent: f64, viewport_h: f64) -> ListViewWidget {
        let mut w = ListViewWidget::new(count, extent);
        w.viewport = Size::new(200.0, viewport_h);
        w
    }

    /// Build and lay out the nested `ScrollView` a row hosts: a
    /// `viewport_h`-tall viewport over 1000px of plain content. Laid out tight
    /// because an enclosing scroll surface hands its child unbounded height,
    /// which would leave this one viewport == content with nothing to scroll —
    /// a real row's own extent is what bounds it, and [`wire_row`] re-applies
    /// exactly that.
    fn nested_scroll(viewport_h: f64) -> ScrollWidget {
        let view: crate::scroll::ScrollView<()> =
            crate::scroll::scroll_view(crate::test_support::leaf(200.0, 1000.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(
            &mut lctx,
            &BoxConstraints::tight(Size::new(200.0, viewport_h)),
        );
        w
    }

    /// Park a nested surface at `offset` px with a wheel scroll (hard-clamped,
    /// no overscroll and no gesture state) — how a real one reaches a
    /// mid-content position.
    fn park_scroll(w: &mut ScrollWidget, viewport_h: f64, offset: f64) {
        let mut unit = ();
        let sa: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, viewport_h));
        w.event(
            &mut ctx,
            &InputEvent::Scroll {
                position: Point::new(10.0, 10.0),
                delta: ScrollDelta::Pixels(0.0, offset),
            },
        );
        assert_eq!(w.offset(), offset, "the fixture parked where it meant to");
    }

    /// Wire `row` in as the list's single materialized row for item `index`,
    /// laid out tight at the list's own item extent so the pod carries the
    /// bounds `route_event` hit-tests against — and so the nested surface gets
    /// the bounded viewport a real row layout hands it.
    fn wire_row(outer: &mut ListViewWidget, index: usize, row: Box<dyn Widget>) {
        let mut pod = ChildPod::new(row);
        let mut lctx = LayoutCtx::new();
        pod.layout_child(
            &mut lctx,
            &BoxConstraints::tight(Size::new(200.0, outer.item_extent)),
        );
        outer.children = vec![pod];
        outer.keys = vec![index];
        outer.sync_child_origins();
    }

    /// The nested surface [`wire_row`] installed as the list's one row.
    fn row_scroll(outer: &ListViewWidget) -> &ScrollWidget {
        (outer.children[0].widget() as &dyn Any)
            .downcast_ref::<ScrollWidget>()
            .expect("the fixture wired a ScrollWidget row")
    }

    fn dispatch_list(w: &mut ListViewWidget, e: &InputEvent, t_ms: f64) {
        let mut unit = ();
        let sa: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(sa, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, e, t_ms);
    }

    #[test]
    fn list_view_outer_defers_when_the_nested_surface_can_consume() {
        // A 200px viewport scrolled to row 2 (content y 400..600, so the row
        // sits exactly over the viewport), that row hosting a scroll surface
        // parked mid-content — at neither of its own edges, so its claim comes
        // from actual room rather than a displacement-allowing physics.
        let mut outer = nested_list(10, 200.0, 200.0);
        outer.offset = 400.0;
        let mut row = nested_scroll(200.0);
        park_scroll(&mut row, 200.0, 300.0);
        wire_row(&mut outer, 2, Box::new(row));

        dispatch_list(&mut outer, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(
            outer.inner_at_down.registered,
            "the row's scroll surface reported itself on the routed Down"
        );
        assert!(outer.inner_at_down.can_consume_up_drag);

        // 50px of finger-up drag, past the slop: the list stands down.
        dispatch_list(&mut outer, &ev(PointerPhase::Move, 50.0), 16.0);
        assert!(outer.deferring);
        assert!(!outer.scrolling, "the list never took the gesture over");
        assert_eq!(outer.offset, 400.0);
        assert!(
            outer.children[0].is_active(),
            "no takeover Cancel went out — the row keeps its capture"
        );

        // The rest of the drag lands in the row, not in the list.
        dispatch_list(&mut outer, &ev(PointerPhase::Move, 10.0), 32.0);
        assert_eq!(outer.offset, 400.0, "the list still has not moved");
        assert_eq!(outer.overscroll, 0.0);
        assert_eq!(
            row_scroll(&outer).offset(),
            340.0,
            "the nested surface consumed the 40px"
        );
    }

    #[test]
    fn list_view_outer_takes_over_when_the_nested_surface_is_pinned() {
        let mut outer = nested_list(10, 200.0, 200.0);
        outer.offset = 400.0;
        let mut row = nested_scroll(200.0);
        // At its own top under a physics that rejects every past-edge
        // proposal: a downward drag has nothing to do there.
        row.physics = Rc::new(crate::physics::parity::Clamping::new());
        wire_row(&mut outer, 2, Box::new(row));

        dispatch_list(&mut outer, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(outer.inner_at_down.registered);
        assert!(!outer.inner_at_down.can_consume_down_drag);

        // 40px down, past the slop: the list takes over exactly as it always
        // has, cancelling the row on the way.
        dispatch_list(&mut outer, &ev(PointerPhase::Move, 140.0), 16.0);
        assert!(outer.scrolling);
        assert!(!outer.deferring);
        assert!(
            !outer.children[0].is_active(),
            "the takeover Cancel released the row's capture"
        );

        dispatch_list(&mut outer, &ev(PointerPhase::Move, 200.0), 32.0);
        assert_eq!(outer.offset, 340.0, "the list consumed the 60px of drag");
        assert_eq!(outer.overscroll, 0.0, "…in range, so no displacement");
        assert_eq!(
            row_scroll(&outer).offset(),
            0.0,
            "the nested surface never moved"
        );
    }

    // --- (9) Keyed reconciliation: row state follows the stable key. ---

    /// Row height and viewport used by every keyed test: 200 / 50 = 4 visible
    /// rows + 2×[`BUFFER`], so a list of ≤ 6 rows materializes whole and a longer
    /// one virtualizes.
    const ROW_EXTENT: f64 = 50.0;
    const KEYED_WINDOW: Size = Size::new(200.0, 200.0);

    /// What a keyed row reports about itself: which row id each live widget
    /// painted, and which rows were torn down.
    #[derive(Default)]
    struct RowLog {
        /// row id -> the build generation of the widget that painted it.
        painted: RefCell<HashMap<u64, u64>>,
        /// row ids whose pods were torn down, in teardown order.
        torn: RefCell<Vec<u64>>,
    }

    impl RowLog {
        /// This frame's `row id -> generation` map (the paint log is cleared at
        /// the top of every keyed frame, so it describes exactly one frame).
        fn painted(&self) -> HashMap<u64, u64> {
            self.painted.borrow().clone()
        }

        fn torn(&self) -> Vec<u64> {
            self.torn.borrow().clone()
        }
    }

    /// A keyed-row fixture. The row carries a stable `id`; its widget is stamped
    /// with a generation from a shared counter (like [`GenView`]) so a relocated —
    /// i.e. state-preserving — row keeps its stamp while a rebuilt-fresh row gets
    /// a new one, and its teardown is recorded.
    ///
    /// This is the host-side stand-in for a hosted `Component`: the generation is
    /// the retained per-row state, and `View::teardown` is exactly the path a
    /// hosted component's owner disposal rides on (`ComponentWidget::teardown`),
    /// so "the removed row's pod was disposed" is observable here without pulling
    /// the reactive runtime into this signal-free crate.
    struct RowView {
        id: u64,
        gens: Rc<Cell<u64>>,
        log: Rc<RowLog>,
    }

    struct RowWidget {
        id: u64,
        generation: u64,
        log: Rc<RowLog>,
    }

    impl View<()> for RowView {
        type Element = RowWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RowWidget {
            let generation = self.gens.get();
            self.gens.set(generation + 1);
            RowWidget {
                id: self.id,
                generation,
                log: self.log.clone(),
            }
        }

        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut RowWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            // A same-type in-place rebuild keeps the generation (state preserved);
            // only the content this row renders is updated.
            element.id = self.id;
            ChangeFlags::NONE
        }

        fn teardown(&self, element: &mut RowWidget, _ctx: &mut BuildCtx<'_>) {
            element.log.torn.borrow_mut().push(element.id);
        }
    }

    impl Widget for RowWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(bc.max())
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.log
                .painted
                .borrow_mut()
                .insert(self.id, self.generation);
        }
    }

    /// A mutable backing vec of row ids plus the shared generation counter and
    /// log — the whole fixture one keyed test drives.
    struct KeyedRows {
        rows: Rc<RefCell<Vec<u64>>>,
        gens: Rc<Cell<u64>>,
        log: Rc<RowLog>,
    }

    impl KeyedRows {
        fn new(ids: Vec<u64>) -> Self {
            Self {
                rows: Rc::new(RefCell::new(ids)),
                gens: Rc::new(Cell::new(0)),
                log: Rc::new(RowLog::default()),
            }
        }

        /// The app-logic closure for a **keyed** list. Each frame captures a fresh
        /// *snapshot* of the row ids into both `key_of` and `builder` — the real
        /// app shape, and the one the reconciler depends on: the retained view of
        /// the previous frame must still reconstruct the *previous* frame's rows
        /// (`prev.builder(prev_index)`), which a shared live handle would not.
        fn keyed_logic(&self) -> impl FnMut(&mut ()) -> ListView<()> + use<> {
            let rows = self.rows.clone();
            let gens = self.gens.clone();
            let log = self.log.clone();
            move |_: &mut ()| {
                let snapshot: Rc<Vec<u64>> = Rc::new(rows.borrow().clone());
                let (keys, items) = (snapshot.clone(), snapshot.clone());
                let (gens, log) = (gens.clone(), log.clone());
                ListView::builder_keyed(
                    snapshot.len(),
                    ROW_EXTENT,
                    move |i| ChildKey::new(keys[i]),
                    move |i| {
                        any::<(), _>(RowView {
                            id: items[i],
                            gens: gens.clone(),
                            log: log.clone(),
                        })
                    },
                )
            }
        }

        /// The same list built **positionally** — the contrast case that pins the
        /// documented index-identity failure the keyed path fixes.
        fn positional_logic(&self) -> impl FnMut(&mut ()) -> ListView<()> + use<> {
            let rows = self.rows.clone();
            let gens = self.gens.clone();
            let log = self.log.clone();
            move |_: &mut ()| {
                let items: Rc<Vec<u64>> = Rc::new(rows.borrow().clone());
                let (gens, log) = (gens.clone(), log.clone());
                ListView::builder(items.len(), ROW_EXTENT, move |i| {
                    any::<(), _>(RowView {
                        id: items[i],
                        gens: gens.clone(),
                        log: log.clone(),
                    })
                })
            }
        }
    }

    /// Drive one frame with the log cleared first, so it describes exactly this
    /// frame's materialized rows and this frame's teardowns.
    fn keyed_frame(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        log: &RowLog,
        ms: f64,
    ) -> ChangeFlags {
        log.painted.borrow_mut().clear();
        log.torn.borrow_mut().clear();
        frame(root, logic, &mut (), KEYED_WINDOW, ms)
    }

    /// Build a root and converge its window (build frame + real-viewport frame).
    fn converged(
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        log: &RowLog,
    ) -> RenderRoot<(), ListView<()>> {
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        keyed_frame(&mut root, logic, log, 0.0);
        keyed_frame(&mut root, logic, log, 16.0);
        root
    }

    #[test]
    fn keyed_mid_list_insert_keeps_each_row_with_its_key() {
        let fx = KeyedRows::new(vec![10, 20, 30, 40, 50]);
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);
        let before = fx.log.painted();
        assert_eq!(before.len(), 5, "all five rows fit the viewport");

        // Insert in the MIDDLE: every row after it shifts one index down.
        fx.rows.borrow_mut().insert(2, 25);
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        let after = fx.log.painted();
        for id in [10, 20, 30, 40, 50] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} kept its own widget (and its state) across the insert"
            );
        }
        let newest = before.values().copied().max().expect("rows painted");
        assert!(
            after[&25] > newest,
            "the inserted row is built fresh, not handed a survivor's widget"
        );
        assert!(
            fx.log.torn().is_empty(),
            "no key left the window, so nothing is torn down"
        );
        assert!(
            flags.contains(ChangeFlags::LAYOUT),
            "a frame that materialized a new pod owes a layout pass"
        );
    }

    #[test]
    fn positional_mid_list_insert_still_misattaches_row_state() {
        // The documented index-identity failure `builder_keyed` exists to fix,
        // pinned so "the positional path is unchanged" is a test, not a claim.
        let fx = KeyedRows::new(vec![10, 20, 30, 40, 50]);
        let mut logic = fx.positional_logic();
        let mut root = converged(&mut logic, &fx.log);
        let before = fx.log.painted();

        fx.rows.borrow_mut().insert(2, 25);
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        let after = fx.log.painted();
        assert_ne!(
            after[&30], before[&30],
            "positional identity leaves row 30's state behind at index 2"
        );
        assert_eq!(
            after[&25], before[&30],
            "and hands it to whatever content now occupies that index"
        );
    }

    #[test]
    fn keyed_mid_list_remove_tears_down_only_the_removed_row() {
        let fx = KeyedRows::new(vec![10, 20, 30, 40, 50]);
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);
        let before = fx.log.painted();

        fx.rows.borrow_mut().remove(2); // drop row 30 from the middle
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        let after = fx.log.painted();
        assert_eq!(
            fx.log.torn(),
            vec![30],
            "exactly the removed row's pod is torn down (its owner disposed)"
        );
        assert!(!after.contains_key(&30), "row 30 no longer paints");
        for id in [10, 20, 40, 50] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} kept its own widget across the removal"
            );
        }
        assert!(
            flags.contains(ChangeFlags::LAYOUT),
            "a frame that tore a pod down owes a layout pass"
        );
    }

    #[test]
    fn keyed_reorder_relocates_rows_and_reports_layout() {
        let fx = KeyedRows::new(vec![10, 20, 30, 40, 50]);
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);
        let before = fx.log.painted();

        fx.rows.borrow_mut().reverse();
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        let after = fx.log.painted();
        for id in [10, 20, 30, 40, 50] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} moved slot but kept its own widget"
            );
        }
        assert!(
            fx.log.torn().is_empty(),
            "a reorder builds and tears down nothing"
        );
        assert!(
            flags.contains(ChangeFlags::LAYOUT),
            "relocated pods sit at new origins, so the frame owes a layout pass"
        );
    }

    #[test]
    fn an_unchanged_keyed_frame_reports_no_layout() {
        // The layout-skip contract in the other direction: a frame that neither
        // built, tore down, relocated, nor re-ranged a pod must NOT force a
        // layout pass, or the idiom degenerates into "relayout every frame".
        let fx = KeyedRows::new(vec![10, 20, 30, 40, 50]);
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);

        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        assert!(
            flags.is_empty(),
            "an identical keyed frame is a no-op reconciliation, got {flags:?}"
        );
    }

    #[test]
    fn keyed_window_shift_relocates_survivors_and_tears_down_the_leaver() {
        // A pure scroll over 1000 keyed rows: virtualization still works, and the
        // retained key map tracks the window it materialized.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);
        let before = fx.log.painted();
        assert_eq!(list_widget(&root).window(), &[0, 1, 2, 3, 4, 5]);

        // Scroll 150px: the window becomes 1..9 — row 0 leaves, rows 6..8 enter.
        root.event(&mut (), &wheel(150.0));
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        let after = fx.log.painted();
        assert_eq!(list_widget(&root).window(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        for id in [10, 20, 30, 40, 50] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} stayed in the window and kept its widget"
            );
        }
        assert_eq!(
            fx.log.torn(),
            vec![0],
            "the row that scrolled out is torn down"
        );
        assert!(
            flags.contains(ChangeFlags::LAYOUT),
            "a shifted window owes a layout pass"
        );
    }

    #[test]
    fn keyed_insert_under_a_scrolled_window_keeps_state_with_the_rows() {
        // The hard case: a mid-list insert *while* the list is virtualized, so
        // every row's index shifts under a window that would otherwise stay
        // put — except this insert is a prepend relative to the scrolled
        // viewport (index 0 sits above it), so scroll anchoring (see the
        // module docs' anchoring section) shifts the *offset* to compensate,
        // keeping the SAME rows on screen rather than sliding new content in.
        // Positional identity has no answer here (see the contrast test
        // above); keys carry each row's state with its index, and anchoring
        // carries the viewport with it too.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);

        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        let before = fx.log.painted();
        assert_eq!(list_widget(&root).window(), &[8, 9, 10, 11, 12, 13, 14, 15]);
        // Rows 80..=150 (ids), one per materialized slot.
        assert_eq!(before.len(), 8);
        let offset_before = list_widget(&root).offset();

        // Insert at the FRONT: every row slides one index down; the anchor
        // (row 80, topmost of the previous window) shifts the offset by
        // exactly one row's extent to hold it in place.
        fx.rows.borrow_mut().insert(0, 5);
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 48.0);

        let after = fx.log.painted();
        assert_eq!(
            list_widget(&root).offset(),
            offset_before + ROW_EXTENT,
            "anchored: the offset absorbs the shift instead of the window sliding"
        );
        for id in [80, 90, 100, 110, 120, 130, 140, 150] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} kept its widget — anchoring shows the SAME rows, not new ones"
            );
        }
        assert!(
            fx.log.torn().is_empty(),
            "anchoring keeps the same window of rows visible: nothing leaves it"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    // --- (10) Prepend/removal scroll anchoring (keyed lists only). ---

    /// The pixel `y` a materialized row paints at (its `ChildPod` origin),
    /// looked up by the item index it currently occupies — the literal paint
    /// position the anchoring assertions below are stated against.
    fn painted_y(w: &ListViewWidget, item_index: usize) -> f64 {
        let slot = w
            .keys
            .iter()
            .position(|&k| k == item_index)
            .expect("index is materialized");
        w.children[slot].origin().y
    }

    #[test]
    fn keyed_prepend_while_scrolled_anchors_the_visible_rows() {
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);

        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        let before = fx.log.painted();
        assert_eq!(list_widget(&root).window(), &[8, 9, 10, 11, 12, 13, 14, 15]);
        let offset_before = list_widget(&root).offset();
        let y_before = painted_y(list_widget(&root), 8); // row id 80's pixel position

        // Prepend 5 rows above the viewport.
        fx.rows
            .borrow_mut()
            .splice(0..0, [9990, 9980, 9970, 9960, 9950]);
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 48.0);

        let after = fx.log.painted();
        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            offset_before + 5.0 * ROW_EXTENT,
            "the offset shifts by exactly K * item_extent"
        );
        assert_eq!(
            painted_y(w, 13), // row id 80 slid from index 8 to index 13
            y_before,
            "the anchor row paints at the exact same pixel position"
        );
        for id in [80, 90, 100, 110, 120, 130, 140, 150] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} kept its widget across the prepend — anchoring, not a rebuild"
            );
        }
        assert!(
            fx.log.torn().is_empty(),
            "anchoring keeps the same rows visible — nothing leaves the window"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn keyed_removal_above_the_viewport_anchors_in_reverse() {
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);

        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        let before = fx.log.painted();
        assert_eq!(list_widget(&root).window(), &[8, 9, 10, 11, 12, 13, 14, 15]);
        let offset_before = list_widget(&root).offset();
        let y_before = painted_y(list_widget(&root), 8);

        // Remove the first 3 rows — all strictly above the viewport.
        fx.rows.borrow_mut().drain(0..3);
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 48.0);

        let after = fx.log.painted();
        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            offset_before - 3.0 * ROW_EXTENT,
            "removal above shifts the offset back by exactly the removed extent"
        );
        assert_eq!(
            painted_y(w, 5), // row id 80 slid from index 8 to index 5
            y_before,
            "the anchor row paints at the exact same pixel position"
        );
        for id in [80, 90, 100, 110, 120, 130, 140, 150] {
            assert_eq!(
                after[&id], before[&id],
                "row {id} kept its widget across the removal"
            );
        }
        assert!(fx.log.torn().is_empty());
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn keyed_removal_above_anchors_correctly_at_max_offset() {
        // Scripted at a third scroll position (0, mid, max_offset) per the
        // task's testing note: the closed-form correction must still clamp
        // correctly when scrolled all the way to the bottom.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);

        root.event(&mut (), &wheel(1_000_000.0)); // scroll to the very bottom
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        let before = fx.log.painted();
        let offset_before = list_widget(&root).offset();
        assert_eq!(offset_before, list_widget(&root).max_offset());

        // Remove the first 3 rows, strictly above the (bottom-scrolled) viewport.
        fx.rows.borrow_mut().drain(0..3);
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 48.0);

        let after = fx.log.painted();
        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            offset_before - 3.0 * ROW_EXTENT,
            "anchors correctly even scrolled to the very bottom"
        );
        assert_eq!(
            w.offset(),
            w.max_offset(),
            "still pinned exactly at the new (smaller) bottom"
        );
        for (&id, &generation) in before.iter() {
            assert_eq!(
                after.get(&id),
                Some(&generation),
                "row {id} kept its widget"
            );
        }
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn keyed_mutation_below_the_viewport_leaves_the_offset_alone() {
        // The third documented case: a below-viewport mutation is a no-op for
        // anchoring (the index the anchor occupies doesn't move at all).
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);

        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        let offset_before = list_widget(&root).offset();

        fx.rows.borrow_mut().truncate(900); // drop the last 100 rows

        keyed_frame(&mut root, &mut logic, &fx.log, 48.0);

        assert_eq!(
            list_widget(&root).offset(),
            offset_before,
            "a below-viewport mutation leaves an already-scrolled offset untouched"
        );
    }

    #[test]
    fn keyed_prepend_at_the_very_top_still_anchors_off_zero() {
        // The module docs' anchor-always decision: even starting at
        // offset == 0 the offset shifts, revealing the new content above
        // rather than staying pinned at the literal top.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);
        assert_eq!(list_widget(&root).offset(), 0.0);

        fx.rows.borrow_mut().splice(0..0, [9990, 9980, 9970]);
        let flags = keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        assert_eq!(
            list_widget(&root).offset(),
            3.0 * ROW_EXTENT,
            "the offset shifts away from zero rather than staying pinned"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn keyed_prepend_near_top_does_not_immediately_refire_near_start() {
        // The edge-latch interaction: a correction must not make the very
        // next event refire an edge that just fired (and whose callback is
        // presumably what triggered the prepend).
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let loads = Rc::new(Cell::new(0u32));
        let loads_l = loads.clone();
        let rows = fx.rows.clone();
        let gens = fx.gens.clone();
        let log = fx.log.clone();
        let mut logic = move |_: &mut ()| -> ListView<()> {
            let snapshot: Rc<Vec<u64>> = Rc::new(rows.borrow().clone());
            let (keys, items) = (snapshot.clone(), snapshot.clone());
            let (gens, log) = (gens.clone(), log.clone());
            let loads = loads_l.clone();
            ListView::builder_keyed(
                snapshot.len(),
                ROW_EXTENT,
                move |i| ChildKey::new(keys[i]),
                move |i| {
                    any::<(), _>(RowView {
                        id: items[i],
                        gens: gens.clone(),
                        log: log.clone(),
                    })
                },
            )
            .on_near_start(move |_: &mut ()| loads.set(loads.get() + 1), 100.0)
        };

        let mut root = converged(&mut logic, &fx.log);
        // A real near-start approach at the top fires once — the trigger a
        // real "load older" flow answers by prepending.
        root.event(&mut (), &wheel(0.0));
        assert_eq!(loads.get(), 1, "near-start fires once at the top");

        // Prepend a single row — the anchor shifts the offset to 1 * extent,
        // well within the 100px threshold: exactly the case that would
        // spuriously refire without the correction's rearm suppression.
        fx.rows.borrow_mut().splice(0..0, [9999]);
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        assert_eq!(
            list_widget(&root).offset(),
            ROW_EXTENT,
            "anchored one row's worth off zero"
        );

        // The very next event, still near the (corrected) start, must NOT
        // refire.
        root.event(&mut (), &wheel(0.0));
        assert_eq!(
            loads.get(),
            1,
            "the corrected offset does not immediately re-fire the same edge"
        );

        // Scrolling away past 2x the threshold and back still rearms normally.
        root.event(&mut (), &wheel(500.0));
        root.event(&mut (), &wheel(-450.0));
        assert_eq!(
            loads.get(),
            2,
            "the edge still rearms after genuinely scrolling away and back"
        );
    }

    #[test]
    fn positional_prepend_does_not_anchor_the_offset() {
        // Criterion 4: positional identity cannot distinguish a prepend from
        // a full mutation, so the positional path must never apply the
        // anchoring correction — the offset moves only from user scrolling.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.positional_logic();
        let mut root = converged(&mut logic, &fx.log);

        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);
        let offset_before = list_widget(&root).offset();

        fx.rows.borrow_mut().insert(0, 12345);
        keyed_frame(&mut root, &mut logic, &fx.log, 48.0);

        assert_eq!(
            list_widget(&root).offset(),
            offset_before,
            "the positional path never corrects the offset"
        );
    }

    // --- (11) Variable extents: estimate + measured-by-key cache. ---

    /// The estimate every variable-extent test declares. Deliberately unequal to
    /// any row's true height, so an unmeasured region is visibly *estimated* and
    /// a measured one visibly is not.
    const ESTIMATE: f64 = 60.0;

    /// Deterministic row height by id: 40/60/80/100/120 px, cycling.
    fn var_height(id: u64) -> f64 {
        40.0 + (id % 5) as f64 * 20.0
    }

    /// Which pass the harness is currently driving — the invocation-context
    /// probe's alphabet.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    enum Pass {
        #[default]
        Idle,
        Rebuild,
        Layout,
        Paint,
    }

    /// The invocation-context probe: the app's builder closure reports the pass
    /// it was called in, so "the builder never runs from layout or paint" (the
    /// wake hazard the module docs call out) is a test, not a claim.
    #[derive(Default)]
    struct BuildProbe {
        pass: Cell<Pass>,
        calls: Cell<usize>,
        violations: RefCell<Vec<Pass>>,
    }

    impl BuildProbe {
        fn note(&self) {
            self.calls.set(self.calls.get() + 1);
            let pass = self.pass.get();
            if pass != Pass::Rebuild {
                self.violations.borrow_mut().push(pass);
            }
        }
    }

    /// A keyed row of a *variable* height (`var_height(id)`), reporting the same
    /// generation/paint/teardown log as [`RowView`]. The height is intrinsic: the
    /// row honors the list's unbounded-height constraint by choosing its own
    /// height, exactly like a real self-sizing row.
    struct VarRowView {
        id: u64,
        gens: Rc<Cell<u64>>,
        log: Rc<RowLog>,
    }

    struct VarRowWidget {
        id: u64,
        generation: u64,
        log: Rc<RowLog>,
    }

    impl View<()> for VarRowView {
        type Element = VarRowWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> VarRowWidget {
            let generation = self.gens.get();
            self.gens.set(generation + 1);
            VarRowWidget {
                id: self.id,
                generation,
                log: self.log.clone(),
            }
        }

        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut VarRowWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.id = self.id;
            ChangeFlags::NONE
        }

        fn teardown(&self, element: &mut VarRowWidget, _ctx: &mut BuildCtx<'_>) {
            element.log.torn.borrow_mut().push(element.id);
        }
    }

    impl Widget for VarRowWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(bc.max().width, var_height(self.id)))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.log
                .painted
                .borrow_mut()
                .insert(self.id, self.generation);
        }
    }

    /// The variable-extent counterpart of [`KeyedRows`]: a mutable backing vec of
    /// ids whose rows size themselves, plus the invocation probe.
    struct VarRows {
        rows: Rc<RefCell<Vec<u64>>>,
        gens: Rc<Cell<u64>>,
        log: Rc<RowLog>,
        probe: Rc<BuildProbe>,
    }

    impl VarRows {
        fn new(ids: Vec<u64>) -> Self {
            Self {
                rows: Rc::new(RefCell::new(ids)),
                gens: Rc::new(Cell::new(0)),
                log: Rc::new(RowLog::default()),
                probe: Rc::new(BuildProbe::default()),
            }
        }

        /// The app-logic closure: a keyed list in variable-extent mode, whose
        /// builder reports every invocation to the probe.
        fn logic(&self) -> impl FnMut(&mut ()) -> ListView<()> + use<> {
            let rows = self.rows.clone();
            let gens = self.gens.clone();
            let log = self.log.clone();
            let probe = self.probe.clone();
            move |_: &mut ()| {
                let snapshot: Rc<Vec<u64>> = Rc::new(rows.borrow().clone());
                let (keys, items) = (snapshot.clone(), snapshot.clone());
                let (gens, log, probe) = (gens.clone(), log.clone(), probe.clone());
                ListView::builder_keyed(
                    snapshot.len(),
                    ESTIMATE,
                    move |i| ChildKey::new(keys[i]),
                    move |i| {
                        probe.note();
                        any::<(), _>(VarRowView {
                            id: items[i],
                            gens: gens.clone(),
                            log: log.clone(),
                        })
                    },
                )
                .estimated_item_extent(ESTIMATE)
            }
        }

        /// [`VarRows::logic`] with the pre-seam [`RubberBand`] feel pinned
        /// explicitly, for the same reason [`rubber_band_logic`] exists.
        fn logic_rubber_band(&self) -> impl FnMut(&mut ()) -> ListView<()> + use<> {
            let mut inner = self.logic();
            move |state: &mut ()| inner(state).physics(RubberBand::new())
        }

        /// The true total content height of the current data.
        fn true_extent(&self) -> f64 {
            self.rows.borrow().iter().copied().map(var_height).sum()
        }
    }

    /// Drive one frame with the probe told which pass is running, returning the
    /// rebuild flags and paint's continuation request.
    fn var_frame(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        fx: &VarRows,
        ms: f64,
    ) -> (ChangeFlags, frust_core::PaintOutcome) {
        fx.log.painted.borrow_mut().clear();
        fx.log.torn.borrow_mut().clear();
        fx.probe.pass.set(Pass::Rebuild);
        let flags = root.rebuild(logic, &mut ());
        fx.probe.pass.set(Pass::Layout);
        root.layout(KEYED_WINDOW);
        fx.probe.pass.set(Pass::Paint);
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::from_nanos((ms * 1_000_000.0) as u64));
        fx.probe.pass.set(Pass::Idle);
        (flags, outcome)
    }

    /// Drive frames until paint stops asking for another one, returning how many
    /// it took. Panics past `limit` — the convergence-frame contract is "a
    /// measurement that changes the window costs *frames*", never a loop.
    fn settle(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        fx: &VarRows,
        first_ms: f64,
        limit: usize,
    ) -> usize {
        for n in 0..limit {
            let (_, outcome) = var_frame(root, logic, fx, first_ms + 16.0 * n as f64);
            if !outcome.needs_frame {
                return n + 1;
            }
        }
        panic!("a variable-extent list did not settle within {limit} frames");
    }

    /// Build a root over a variable-extent list and settle its first window.
    fn converged_var(
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        fx: &VarRows,
    ) -> RenderRoot<(), ListView<()>> {
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        settle(&mut root, logic, fx, 0.0, 6);
        root
    }

    /// Assert the materialized rows tile the content with no gap and no overlap,
    /// and that they cover the whole viewport — the "cumulative extents minus
    /// offset" property, read off the actual pod origins/sizes.
    fn assert_tiles_and_covers(w: &ListViewWidget) {
        assert!(!w.children.is_empty(), "something must be materialized");
        for slot in 0..w.children.len() {
            let pod = &w.children[slot];
            let id = w.keys[slot] as u64;
            assert_eq!(
                pod.size().height,
                var_height(id),
                "row {id} laid out at its own intrinsic height"
            );
            assert_eq!(
                pod.origin().y,
                w.slot_y[slot] - w.offset(),
                "row {id} paints at its content position minus the offset"
            );
            if slot + 1 < w.children.len() {
                assert_eq!(
                    w.children[slot + 1].origin().y,
                    pod.origin().y + pod.size().height,
                    "rows tile: no gap, no overlap between slots {slot} and {}",
                    slot + 1
                );
            }
        }
        let first = w.children[0].origin().y;
        let last = w.children.last().expect("non-empty");
        let bottom = last.origin().y + last.size().height;
        assert!(
            first <= 0.0,
            "the first materialized row starts at or above the viewport top (got {first})"
        );
        assert!(
            bottom >= w.viewport.height,
            "the materialized window reaches the viewport bottom (got {bottom})"
        );
    }

    #[test]
    fn variable_rows_tile_at_their_own_measured_heights() {
        let fx = VarRows::new((0..60).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);
        assert_tiles_and_covers(list_widget(&root));

        // Scroll into a region no row has been measured in yet, settle, and the
        // same tiling property must hold there.
        root.event(&mut (), &wheel(700.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);
        assert_tiles_and_covers(list_widget(&root));

        // ...and back up over now-measured territory.
        root.event(&mut (), &wheel(-300.0));
        settle(&mut root, &mut logic, &fx, 200.0, 4);
        assert_tiles_and_covers(list_widget(&root));
    }

    #[test]
    fn variable_scroll_into_unmeasured_territory_converges_in_one_frame() {
        let fx = VarRows::new((0..200).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        // A wheel jump far past anything measured: the next rebuild windows from
        // the estimate, layout measures what it materialized, and at most one
        // convergence frame re-windows against the revised extents.
        root.event(&mut (), &wheel(2_000.0));
        let frames = settle(&mut root, &mut logic, &fx, 100.0, 2);
        assert!(
            frames <= 2,
            "a scroll into unmeasured territory converges within one extra frame (took {frames})"
        );
        assert_tiles_and_covers(list_widget(&root));
        assert!(
            fx.probe.violations.borrow().is_empty(),
            "the builder ran outside rebuild: {:?}",
            fx.probe.violations.borrow()
        );
    }

    #[test]
    fn the_builder_never_runs_outside_rebuild_even_under_a_fling() {
        // The same probe across the hardest path: a drag/fling that advances the
        // offset *at paint* across unmeasured rows. Paint must materialize
        // nothing — it may only ask for another frame.
        let fx = VarRows::new((0..400).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &ev(PointerPhase::Down, 180.0));
        var_frame(&mut root, &mut logic, &fx, 100.0);
        root.event(&mut (), &ev(PointerPhase::Move, 120.0)); // takeover
        var_frame(&mut root, &mut logic, &fx, 116.0);
        root.event(&mut (), &ev(PointerPhase::Move, 40.0)); // build velocity
        root.event(&mut (), &ev(PointerPhase::Up, 40.0)); // release → fling
        assert!(list_widget(&root).is_flinging());

        for k in 0..12 {
            var_frame(&mut root, &mut logic, &fx, 132.0 + 16.0 * k as f64);
        }

        assert!(fx.probe.calls.get() > 0, "the probe saw the builder at all");
        assert!(
            fx.probe.violations.borrow().is_empty(),
            "the builder ran outside rebuild: {:?}",
            fx.probe.violations.borrow()
        );
        // The fling crossed unmeasured rows without panicking or stalling.
        assert!(list_widget(&root).offset() > 0.0);
    }

    #[test]
    fn uniform_mode_takes_the_closed_form_path_and_measures_nothing() {
        // Criterion 3: without `estimated_item_extent` nothing about the extent
        // model changes — no measurement is cached, no per-slot geometry is
        // retained, and rows sit at exactly `index * item_extent`.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut logic = fx.keyed_logic();
        let mut root = converged(&mut logic, &fx.log);
        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut logic, &fx.log, 32.0);

        let w = list_widget(&root);
        assert!(w.estimated_extent.is_none(), "no estimate was declared");
        assert!(!w.is_variable(), "the uniform path is taken");
        assert!(w.measured.is_empty(), "the measured cache stays empty");
        assert_eq!(w.measured_sum, 0.0);
        assert!(w.slot_keys.is_empty() && w.slot_y.is_empty());
        assert_eq!(
            w.max_offset(),
            1000.0 * ROW_EXTENT - w.viewport.height,
            "the closed-form scroll extent is unchanged"
        );
        assert_eq!(
            painted_y(w, 10),
            10.0 * ROW_EXTENT - w.offset(),
            "rows sit at the closed-form position"
        );
    }

    #[test]
    fn variable_total_extent_converges_to_the_true_sum() {
        use frust_core::accesskit::Role;

        let fx = VarRows::new((0..30).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        // Before anything is visited the extent is mostly estimated.
        let estimated_total = list_widget(&root).content_extent();
        assert!(
            (estimated_total - fx.true_extent()).abs() > 1.0,
            "the estimate and the truth differ to begin with"
        );

        // Walk the whole list a viewport at a time, so every row is materialized
        // (and therefore measured) at least once.
        for step in 0..40 {
            root.event(&mut (), &wheel(100.0));
            var_frame(&mut root, &mut logic, &fx, 100.0 + 16.0 * step as f64);
        }

        let w = list_widget(&root);
        assert_eq!(w.measured.len(), 30, "every row was visited and measured");
        assert!(
            (w.content_extent() - fx.true_extent()).abs() < 1e-6,
            "the total extent converged to Σ true extents: {} vs {}",
            w.content_extent(),
            fx.true_extent()
        );
        assert!(
            (w.max_offset() - (fx.true_extent() - w.viewport.height)).abs() < 1e-6,
            "max_offset tracks the converged extent"
        );

        let update = root.semantics();
        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("the ListView contributes a Role::List container node");
        assert_eq!(
            list.scroll_y_max(),
            Some(fx.true_extent() - KEYED_WINDOW.height),
            "the semantics scroll range tracks it too"
        );
    }

    #[test]
    fn variable_cache_evicts_a_removed_row_but_keeps_a_scrolled_away_one() {
        let fx = VarRows::new((0..40).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);
        assert!(
            list_widget(&root)
                .measured
                .contains_key(&ChildKey::new(0u64)),
            "row 0 was measured while it was on screen"
        );

        // Scroll it out of the window: it left the *window*, not the data, so its
        // measurement must survive (this is what makes the extent converge).
        root.event(&mut (), &wheel(400.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);
        let w = list_widget(&root);
        assert!(!w.window().contains(&0), "row 0 is no longer materialized");
        assert!(
            w.measured.contains_key(&ChildKey::new(0u64)),
            "a row that only left the window keeps its measurement"
        );

        // Now remove a row that IS in the window: its key left the data, so its
        // measurement is evicted and the running sum drops by exactly its height.
        let victim = list_widget(&root).window()[3] as u64;
        let sum_before = list_widget(&root).measured_sum;
        let count_before = list_widget(&root).measured.len();
        fx.rows.borrow_mut().retain(|&id| id != victim);
        root.rebuild(&mut logic, &mut ()); // rebuild alone: no re-measurement yet

        let w = list_widget(&root);
        assert!(
            !w.measured.contains_key(&ChildKey::new(victim)),
            "the removed row's measurement is evicted"
        );
        assert_eq!(w.measured.len(), count_before - 1);
        assert!(
            (w.measured_sum - (sum_before - var_height(victim))).abs() < 1e-9,
            "the running sum drops by exactly the evicted extent"
        );
    }

    #[test]
    fn variable_full_replace_clears_the_measured_cache() {
        let fx = VarRows::new((0..40).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);
        assert!(!list_widget(&root).measured.is_empty());

        // Every id replaced: no previous-window key survives either hypothesis,
        // which is reset semantics for the cache as well as for anchoring.
        *fx.rows.borrow_mut() = (0..40).map(|i| 10_000 + i).collect();
        root.rebuild(&mut logic, &mut ());

        let w = list_widget(&root);
        assert!(w.measured.is_empty(), "a full replace clears the cache");
        assert_eq!(w.measured_sum, 0.0);
    }

    #[test]
    fn same_frame_prepend_and_append_leaves_the_measured_cache_intact() {
        // A same-frame mutation on BOTH sides of the anchor (a prepend above
        // the viewport plus an append below it, in one keyed frame) is a
        // false negative for the rebuild-time anchor-shift probe: the net
        // `item_count` delta is +4 (3 prepended, 1 appended) but the true
        // per-row shift for every already-existing row is +3 (the append
        // never shifts an existing index), so the probe's two hypotheses
        // (unchanged position, or shifted by the net delta) both miss.
        //
        // Before this fix that bare miss also wiped the whole measured
        // cache (`ListView::rebuild`'s `None` arm called `clear_measured`
        // unconditionally). This pins the fix: the scroll-position
        // correction itself is knowingly still absent this frame (a visible
        // jump — the exact per-key anchor fix is a deferred follow-up, see
        // the module docs' *Cache hygiene* section), but the wholesale clear
        // no longer fires, because `reconcile_keyed` still matches *some*
        // on-screen rows into this frame's window by *exact* key, not by the
        // probe's hypotheses (`any_survivor`).
        //
        // Not every previously on-screen row is such a survivor, though: the
        // window itself is recomputed against the same (uncorrected, since
        // the probe missed) offset over now-shifted content, so a row whose
        // slot falls outside the recomputed window becomes an orphan the
        // main reconcile loop never claims. The departing-row eviction loop
        // (`ListView::reconcile_keyed`'s *Cache hygiene* section) tests that
        // orphan against the same two hypotheses the probe already found
        // unreliable this frame, and — per the module docs' *acknowledged
        // gaps* — conservatively evicts it rather than leaving it stale
        // forever: a merely-window-departed row costs one re-measurement on
        // its next visit; a genuinely data-departed row (a distinct
        // scenario pinned by
        // `probe_miss_with_survivor_evicts_a_row_that_left_the_data`) would
        // otherwise leak permanently. Both outcomes are asserted below,
        // derived from which rows the reconciled window actually still
        // names — not hardcoded — so this test tracks the real reconciliation
        // rather than one snapshot of its internals.
        let fx = VarRows::new((0..60).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        // Scroll to a middle window so there is real content both above and
        // below what is materialized.
        root.event(&mut (), &wheel(400.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);

        let w = list_widget(&root);
        let on_screen_ids: Vec<u64> = w.window().iter().map(|&i| fx.rows.borrow()[i]).collect();
        assert!(!on_screen_ids.is_empty(), "the window materialized rows");
        let measured_before: HashMap<u64, f64> = on_screen_ids
            .iter()
            .map(|&id| {
                let m = w.measured[&ChildKey::new(id)];
                (id, m.extent)
            })
            .collect();
        assert_eq!(
            measured_before.len(),
            on_screen_ids.len(),
            "every on-screen row was already measured, laid out once by `converged_var`"
        );

        // Prepend 3 above and append 1 below in one borrow_mut batch, so both
        // mutations land in the same keyed rebuild.
        {
            let mut rows = fx.rows.borrow_mut();
            rows.splice(0..0, [9_001u64, 9_002, 9_003]);
            rows.push(9_050);
        }
        root.rebuild(&mut logic, &mut ()); // rebuild alone: pins cache state, not paint/layout

        let w = list_widget(&root);
        let new_window_ids: std::collections::HashSet<u64> =
            w.window().iter().map(|&i| fx.rows.borrow()[i]).collect();
        assert!(
            on_screen_ids.iter().any(|id| new_window_ids.contains(id)),
            "at least one previously on-screen row is still reconciled into this \
             frame's window by exact key — `any_survivor`, the precondition that \
             keeps the wholesale clear from firing"
        );

        let mut survivors = 0;
        let mut departed = 0;
        for (id, extent_before) in &measured_before {
            let entry = w.measured.get(&ChildKey::new(*id));
            if new_window_ids.contains(id) {
                survivors += 1;
                assert!(
                    entry.is_some(),
                    "row {id} is still named by this frame's reconciled window \
                     (an exact key match), so its measurement must survive"
                );
                assert_eq!(
                    entry.unwrap().extent,
                    *extent_before,
                    "row {id}'s measured height is unchanged"
                );
            } else {
                departed += 1;
                assert!(
                    entry.is_none(),
                    "row {id} left this frame's reconciled window and neither \
                     departing-row hypothesis re-explains its new position — the \
                     conservative eviction this fix restores (re-measured on its \
                     next visit, never a permanent leak)"
                );
            }
        }
        assert!(survivors > 0, "the survivor assertion above is exercised");
        assert!(
            departed > 0,
            "the window-departure assertion above is exercised — if this ever \
             stops firing (e.g. a wider window swallows the whole shift), widen \
             the mutation so the same-frame-both-sides mismatch still orphans a \
             row and this test keeps covering both outcomes"
        );
    }

    #[test]
    fn probe_miss_with_survivor_evicts_a_row_that_left_the_data() {
        // A same-frame mutation on both sides of the anchor (prepend above
        // the viewport, remove the topmost on-screen row itself, append
        // below) is the same false-negative shape
        // `same_frame_prepend_and_append_leaves_the_measured_cache_intact`
        // pins for the anchor-shift probe: removing the *first* on-screen
        // row means every remaining on-screen row's actual shift (+2: +3
        // from the prepend, −1 from the row removed ahead of it) differs
        // from the frame's net `item_count` delta (+3: +3 prepended, −1
        // removed, +1 appended), so the probe's two hypotheses (unchanged
        // position, or shifted by the net delta) miss for every key in the
        // previous window — `anchor_probe_missed`.
        //
        // Unlike that test, `reconcile_keyed`'s own exact per-slot lookup
        // still finds *some* of those rows in the new window
        // (`any_survivor`) — the precondition this test targets: a
        // probe-miss frame that is *not* a wholesale replace, so the
        // measured cache's own wholesale reset stays off. But the removed
        // row genuinely left the *data*, not merely the window, and must
        // still be evicted by the per-row eviction loop this fix restores —
        // left alone it would leak forever, since `record_measurement`
        // never revives a dead key and this frame's *growing* `item_count`
        // never triggers the shrink-only eviction rule either.
        let fx = VarRows::new((0..60).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);
        root.event(&mut (), &wheel(400.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);

        let w = list_widget(&root);
        let on_screen_ids: Vec<u64> = w.window().iter().map(|&i| fx.rows.borrow()[i]).collect();
        assert!(
            on_screen_ids.len() > 1,
            "the window materialized more than the victim alone"
        );
        let victim = on_screen_ids[0];
        let offset_before = w.offset();
        let count_before = w.measured.len();
        let sum_before = w.measured_sum;
        assert!(
            w.measured.contains_key(&ChildKey::new(victim)),
            "the victim was already measured, laid out once by `converged_var`"
        );

        // Prepend 3 above the viewport, remove the topmost on-screen row
        // itself, and append 1 below — all in one keyed rebuild.
        {
            let mut rows = fx.rows.borrow_mut();
            rows.splice(0..0, [9_001u64, 9_002, 9_003]);
            rows.retain(|&id| id != victim);
            rows.push(9_050);
        }
        root.rebuild(&mut logic, &mut ()); // rebuild alone: pins cache state, not paint/layout

        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            offset_before,
            "the anchor probe missed, so no anchor-shift correction ran this \
             frame — the offset a genuine prepend/removal would anchor is \
             untouched here"
        );

        let new_window_ids: std::collections::HashSet<u64> =
            w.window().iter().map(|&i| fx.rows.borrow()[i]).collect();
        assert!(
            on_screen_ids[1..]
                .iter()
                .any(|id| new_window_ids.contains(id)),
            "at least one other previously on-screen row is still reconciled \
             into this frame's window by exact key — `any_survivor`, the \
             precondition that keeps the wholesale clear from firing"
        );
        for id in &on_screen_ids[1..] {
            if new_window_ids.contains(id) {
                assert!(
                    w.measured.contains_key(&ChildKey::new(*id)),
                    "row {id} is still named by this frame's reconciled \
                     window, so its measurement must survive"
                );
            }
        }

        assert!(
            !w.measured.contains_key(&ChildKey::new(victim)),
            "the removed row's measurement is evicted that same frame, even \
             though the anchor probe missed — this fix's whole point"
        );
        assert!(
            w.measured.len() <= count_before,
            "the cache never grows across a rebuild-only pass over a removal"
        );
        assert!(
            (w.measured_sum - w.measured.values().map(|m| m.extent).sum::<f64>()).abs() < 1e-9,
            "the running sum still matches the surviving entries — \
             `measured_sum` and `measured` stay consistent, no leak"
        );
        assert!(
            w.measured_sum < sum_before,
            "the sum strictly dropped (at least the victim's own extent left it)"
        );
    }

    #[test]
    fn variable_truncation_evicts_measurements_past_the_new_end() {
        let fx = VarRows::new((0..40).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        // Visit the far end so those rows carry measurements, then drop them from
        // the data while the viewport is nowhere near them.
        root.event(&mut (), &wheel(2_000.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);
        assert!(list_widget(&root).measured.len() > 10);

        fx.rows.borrow_mut().truncate(5);
        root.rebuild(&mut logic, &mut ());

        let w = list_widget(&root);
        assert!(
            w.measured.values().all(|m| m.index < 5),
            "every entry the shrunk keying cannot produce is gone"
        );
        assert!(
            (w.measured_sum - w.measured.values().map(|m| m.extent).sum::<f64>()).abs() < 1e-9,
            "the running sum still matches the surviving entries"
        );
    }

    #[test]
    fn variable_prepend_anchors_by_the_estimate() {
        // Anchoring (the previous step) still holds in variable-extent mode: the
        // prepended rows are unmeasured by definition, so the correction — and
        // the prefix anchor it moves with it — steps by the estimate.
        let fx = VarRows::new((0..200).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(600.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);
        let anchor_id = list_widget(&root).window()[0] as u64;
        let offset_before = list_widget(&root).offset();
        let y_before = list_widget(&root).children[0].origin().y;
        let painted_before = fx.log.painted();

        fx.rows.borrow_mut().splice(0..0, [9001, 9002, 9003]);
        let (flags, _) = var_frame(&mut root, &mut logic, &fx, 200.0);

        let w = list_widget(&root);
        assert_eq!(
            w.offset(),
            offset_before + 3.0 * ESTIMATE,
            "the offset absorbs three unmeasured rows' worth of prepend"
        );
        assert_eq!(
            w.children[0].origin().y,
            y_before,
            "the anchor row paints at the same pixel position"
        );
        assert_eq!(w.keys[0] as u64, anchor_id + 3, "shifted by three indices");
        assert_eq!(
            fx.log.painted()[&anchor_id],
            painted_before[&anchor_id],
            "and kept its own widget"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn an_unchanged_variable_frame_reports_no_layout_once_settled() {
        // The layout-skip contract survives the extent model: once measurements
        // have settled, an identical frame must be a no-op reconciliation, or
        // variable extents would degenerate into "relayout every frame".
        //
        // Paint's convergence check asks for another frame only while the
        // materialized window fails to *cover* the viewport, so a window that
        // measured out *wider* than it needs to be is trimmed by the next
        // rebuild instead — one more structural frame after `settle` returns,
        // then steady. That trim frame is the one allowance here.
        let fx = VarRows::new((0..60).collect());
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);
        root.event(&mut (), &wheel(500.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);
        var_frame(&mut root, &mut logic, &fx, 200.0); // the trim frame

        for k in 0..4 {
            let (flags, outcome) = var_frame(&mut root, &mut logic, &fx, 216.0 + 16.0 * k as f64);
            assert!(
                flags.is_empty(),
                "identical variable-extent frame {k} is a no-op reconciliation, got {flags:?}"
            );
            assert!(
                !outcome.needs_frame,
                "and it does not keep asking for another frame"
            );
        }
    }

    #[test]
    fn variable_short_and_empty_lists_are_well_behaved() {
        // Content shorter than the viewport pins the offset at zero and still
        // tiles from measured heights; an empty list materializes nothing and
        // never walks a prefix at all.
        let fx = VarRows::new(vec![0, 1, 2]); // 40 + 60 + 80 = 180 < 200
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        let w = list_widget(&root);
        assert_eq!(w.window(), &[0, 1, 2]);
        assert_eq!(
            w.max_offset(),
            0.0,
            "content shorter than the viewport does not scroll"
        );
        assert_eq!(w.children[0].origin().y, 0.0);
        assert_eq!(
            w.children[2].origin().y,
            100.0,
            "rows 0 and 1 (40 + 60) stack above row 2"
        );

        fx.rows.borrow_mut().clear();
        settle(&mut root, &mut logic, &fx, 100.0, 3);
        assert!(list_widget(&root).window().is_empty());
        assert_eq!(list_widget(&root).max_offset(), 0.0);
        assert!(list_widget(&root).measured.is_empty());
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "requires ListView::builder_keyed")]
    fn estimated_item_extent_needs_a_keyed_list() {
        // Variable extents cache a measurement under row identity; a positional
        // list has none. Debug trips the tripwire; release ignores the estimate
        // and stays uniform (documented on the builder method).
        let _ = list_view(10, ROW_EXTENT, |i| any::<(), _>(gen_stub(i)))
            .estimated_item_extent(ROW_EXTENT);
    }

    // Duplicate keys are ambiguous: the debug tripwire is the contract (release
    // carries on with first-claim-wins, documented on `builder_keyed`).

    // --- (12) Measured anchor correction + the fling's accumulate-then-apply. ---

    /// A row whose id is `≡ 3 (mod 5)` measures [`TALL`] — well above
    /// [`ESTIMATE`] — and one `≡ 0 (mod 5)` measures [`SHORT`], well below it
    /// ([`var_height`] cycles 40/60/80/100/120 by `id % 5`). Stepping ids by 5
    /// therefore builds a list of one known height, in either direction away
    /// from the estimate.
    const TALL: f64 = 100.0;
    const SHORT: f64 = 40.0;

    fn tall_ids(n: usize) -> Vec<u64> {
        (0..n).map(|i| i as u64 * 5 + 3).collect()
    }

    fn short_ids(n: usize) -> Vec<u64> {
        (0..n).map(|i| i as u64 * 5).collect()
    }

    /// The topmost row the viewport actually shows — the anchor a correction
    /// holds still — as `(row id, painted y)`, read straight off the live pods.
    fn anchor_row(w: &ListViewWidget, fx: &VarRows) -> (u64, f64) {
        let rows = fx.rows.borrow();
        for (slot, pod) in w.children.iter().enumerate() {
            if pod.origin().y + pod.size().height > 0.0 {
                return (rows[w.keys[slot]], pod.origin().y);
            }
        }
        panic!("no materialized row reaches the viewport top");
    }

    #[test]
    fn measured_correction_holds_the_anchor_when_rows_measure_taller() {
        // Criterion 1, upward: a jump lands in a region whose rows all measure
        // 100 against a 60px estimate, so the two rows the window buffers above
        // the viewport top measure taller than the offset was planned against.
        let fx = VarRows::new(tall_ids(300));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(1_500.0));
        var_frame(&mut root, &mut logic, &fx, 100.0);

        let w = list_widget(&root);
        let pending = w.pending_correction;
        let offset_before = w.offset();
        let (anchor_id, anchor_y) = anchor_row(w, &fx);
        assert!(
            pending > 0.0,
            "the rows above the viewport top measured taller than assumed"
        );

        // The convergence frame commits it, and the anchor row is painted at the
        // pixel position it already occupied — before *and* after.
        let (flags, outcome) = var_frame(&mut root, &mut logic, &fx, 116.0);
        let w = list_widget(&root);
        assert_eq!(w.pending_correction, 0.0, "the correction was committed");
        assert!(
            (w.offset() - (offset_before + pending)).abs() < 1e-9,
            "the offset absorbed exactly the measured delta"
        );
        let (id_after, y_after) = anchor_row(w, &fx);
        assert_eq!(id_after, anchor_id, "the same row is still at the top");
        assert!(
            (y_after - anchor_y).abs() < 1e-9,
            "the anchor row's painted y is unchanged across the correction \
             ({anchor_y} -> {y_after})"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(!outcome.needs_frame, "one convergence frame, then settled");
    }

    #[test]
    fn measured_correction_holds_the_anchor_when_rows_measure_shorter() {
        // Criterion 1, the other direction: rows measure 40 against the same
        // 60px estimate, so the correction is negative and the offset moves
        // *back* to hold the anchor still.
        let fx = VarRows::new(short_ids(300));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(1_500.0));
        var_frame(&mut root, &mut logic, &fx, 100.0);

        let w = list_widget(&root);
        let pending = w.pending_correction;
        let offset_before = w.offset();
        let (anchor_id, anchor_y) = anchor_row(w, &fx);
        assert_eq!(
            w.children[0].size().height,
            SHORT,
            "the fixture's rows really do measure shorter than the estimate"
        );
        assert!(
            pending < 0.0,
            "the rows above the viewport top measured shorter than assumed"
        );

        let (flags, _) = var_frame(&mut root, &mut logic, &fx, 116.0);
        let w = list_widget(&root);
        assert_eq!(w.pending_correction, 0.0);
        assert!((w.offset() - (offset_before + pending)).abs() < 1e-9);
        let (id_after, y_after) = anchor_row(w, &fx);
        assert_eq!(id_after, anchor_id);
        assert!(
            (y_after - anchor_y).abs() < 1e-9,
            "the anchor row's painted y is unchanged across the correction \
             ({anchor_y} -> {y_after})"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn full_replace_after_a_pending_correction_discards_it_instead_of_committing_it() {
        // Wheel-jump into unmeasured tall rows so a real frame leaves
        // `pending_correction > 0` — the same idiom
        // `measured_correction_holds_the_anchor_when_rows_measure_taller` and
        // `drag_takeover_seeds_from_raw_offset_not_the_pending_corrected_painted_offset`
        // use to produce a genuine (not synthetic) correction, rather than
        // committing it on the very next ordinary frame.
        let fx = VarRows::new(tall_ids(300));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(1_500.0));
        var_frame(&mut root, &mut logic, &fx, 100.0);

        let w = list_widget(&root);
        let pending = w.pending_correction;
        let offset_before = w.offset();
        assert!(
            pending > 0.0,
            "the rows above the viewport top measured taller than assumed"
        );

        // Instead of an ordinary next frame (which would commit `pending`),
        // replace every id: no previous-window key survives either
        // hypothesis, a genuine full replace exactly like
        // `variable_full_replace_clears_the_measured_cache` — reset
        // semantics for the measured cache *and* for anchoring, on a frame
        // that also happens to be carrying a stale, now-unexplainable
        // correction computed against geometry this list no longer holds.
        *fx.rows.borrow_mut() = (0..300).map(|i| 10_000 + i).collect();
        root.rebuild(&mut logic, &mut ()); // rebuild alone: pins offset/cache state

        let w = list_widget(&root);
        assert_eq!(
            w.pending_correction, 0.0,
            "the stale correction is discarded, not left pending"
        );
        assert_eq!(
            w.offset(),
            offset_before,
            "the offset did NOT absorb the stale correction — `item_count` is \
             unchanged (300 before and after) and {offset_before} is already \
             far inside `max_offset` either way, so the only way this could \
             differ is the bug this pins: committing `pending` (giving \
             {}) into geometry the full replace made unexplainable",
            offset_before + pending
        );
        assert!(
            w.measured.is_empty(),
            "a full replace clears the measured cache too (the round-0 fix, \
             still intact — `reconcile_keyed`'s wholesale-clear branch)"
        );
        assert_eq!(w.measured_sum, 0.0);
    }

    #[test]
    fn drag_takeover_seeds_from_raw_offset_not_the_pending_corrected_painted_offset() {
        // Wheel-jump into unmeasured tall rows so a real frame leaves
        // `pending_correction > 0` — mirrors
        // `measured_correction_holds_the_anchor_when_rows_measure_taller`'s
        // idiom for producing a genuine (not synthetic) pending correction.
        let fx = VarRows::new(tall_ids(300));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(1_500.0));
        var_frame(&mut root, &mut logic, &fx, 100.0);

        let w = list_widget(&root);
        let pending = w.pending_correction;
        assert!(
            pending > 0.0,
            "the rows above the viewport top measured taller than assumed"
        );
        let painted_before = w.painted_offset();

        // WITHOUT another rebuild: Down -> Move past TOUCH_SLOP (takeover) ->
        // a second Move with a known finger delta `dy`.
        let down_y = 100.0;
        root.event(&mut (), &ev(PointerPhase::Down, down_y));
        let takeover_y = down_y + TOUCH_SLOP + 1.0;
        root.event(&mut (), &ev(PointerPhase::Move, takeover_y)); // takeover
        assert!(
            list_widget(&root).scrolling,
            "the slop crossing took the gesture over"
        );
        assert_eq!(
            list_widget(&root).pending_correction,
            pending,
            "takeover itself never touches pending_correction"
        );

        let dy = 30.0;
        root.event(&mut (), &ev(PointerPhase::Move, takeover_y + dy));

        let w = list_widget(&root);
        assert!(
            (w.painted_offset() - (painted_before - dy)).abs() < 1e-9,
            "painted offset after the second Move equals the pre-takeover \
             painted offset minus exactly the finger delta \
             ({painted_before} - {dy} = {}, got {})",
            painted_before - dy,
            w.painted_offset()
        );
        assert_eq!(
            w.pending_correction, pending,
            "the drag path never commits or otherwise touches pending_correction \
             (only a rebuild does — see `ListViewWidget::apply_pending_correction`)"
        );
    }

    #[test]
    fn variable_prepend_refines_from_the_estimate_to_the_measurement() {
        // Criterion 2's two steps, in consecutive frames: reconciliation shifts
        // by the estimate (a prepended row is unmeasured by definition), then
        // the frame that measures those rows refines the shift to the truth —
        // with the anchor row painting at the same y at every step.
        let fx = VarRows::new(tall_ids(60));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        // Half a row down: the viewport top sits inside row 0, so a prepend
        // lands *inside* the materialized window and is measurable at all (a
        // prepend far above the window can only ever be estimated — see the
        // module docs' *What stays estimated*).
        root.event(&mut (), &wheel(TALL / 2.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);
        let w = list_widget(&root);
        let (anchor_id, anchor_y) = anchor_row(w, &fx);
        let offset_before = w.offset();

        // Step one: the estimate-based shift.
        fx.rows.borrow_mut().splice(0..0, [5_003u64, 5_008u64]);
        var_frame(&mut root, &mut logic, &fx, 200.0);
        let w = list_widget(&root);
        assert!(
            (w.offset() - (offset_before + 2.0 * ESTIMATE)).abs() < 1e-9,
            "reconciliation shifts by the estimate first"
        );
        assert!(
            (w.pending_correction - 2.0 * (TALL - ESTIMATE)).abs() < 1e-9,
            "and layout records what the two prepended rows actually measured"
        );
        let (id_mid, y_mid) = anchor_row(w, &fx);
        assert_eq!(id_mid, anchor_id);
        assert!(
            (y_mid - anchor_y).abs() < 1e-9,
            "the anchor row did not move on the reconciling frame"
        );

        // Step two: the measured refinement.
        let (flags, _) = var_frame(&mut root, &mut logic, &fx, 216.0);
        let w = list_widget(&root);
        assert_eq!(w.pending_correction, 0.0);
        assert!(
            (w.offset() - (offset_before + 2.0 * TALL)).abs() < 1e-9,
            "the offset ends up shifted by the prepended rows' *true* extent"
        );
        let (id_after, y_after) = anchor_row(w, &fx);
        assert_eq!(id_after, anchor_id, "still the same row at the top");
        assert!(
            (y_after - anchor_y).abs() < 1e-9,
            "and it never moved across either step"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    /// Fling **upward** (finger dragging down) over rows nothing has measured,
    /// pumping frames until the fling comes to rest. Returns one `(offset, still
    /// flinging, pending correction)` sample per frame, starting at the release.
    ///
    /// Upward is the interesting direction: rows enter the window *above* the
    /// viewport top, so every frame measures rows that correct the offset —
    /// against the fling when they measure taller than the estimate.
    fn fling_up_and_pump(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        fx: &VarRows,
        first_ms: f64,
    ) -> Vec<(f64, bool, f64)> {
        root.event(&mut (), &ev(PointerPhase::Down, 20.0));
        var_frame(root, logic, fx, first_ms);
        root.event(&mut (), &ev(PointerPhase::Move, 90.0)); // takeover
        var_frame(root, logic, fx, first_ms + 16.0);
        root.event(&mut (), &ev(PointerPhase::Move, 190.0)); // build velocity
        root.event(&mut (), &ev(PointerPhase::Up, 190.0)); // release
        assert!(
            list_widget(root).is_flinging(),
            "the release started a fling"
        );

        let w = list_widget(root);
        let mut trace = vec![(w.offset(), true, w.pending_correction)];
        for k in 0..300 {
            var_frame(root, logic, fx, first_ms + 32.0 + 16.0 * k as f64);
            let w = list_widget(root);
            trace.push((w.offset(), w.is_flinging(), w.pending_correction));
            if !w.is_flinging() {
                return trace;
            }
        }
        panic!("the fling never came to rest");
    }

    /// Assert a fling trajectory only ever moved in the fling's own direction —
    /// the monotonicity a per-frame correction against a paint-advancing offset
    /// would break.
    fn assert_monotonic_upward(trace: &[(f64, bool, f64)]) {
        for pair in trace.windows(2) {
            let (before, _, _) = pair[0];
            let (after, _, _) = pair[1];
            assert!(
                after <= before,
                "the offset stepped backwards during an upward fling \
                 ({before} -> {after})"
            );
        }
    }

    #[test]
    fn an_upward_fling_accumulates_the_correction_and_applies_it_at_settle() {
        // Criterion 3. Every frame of this fling pulls unmeasured rows in above
        // the viewport top and measures them *taller*, i.e. a correction
        // pointing against the fling: withheld, the trajectory is the fling's
        // alone; applied per frame it would step backwards once the decaying
        // fling advance drops under the per-frame correction.
        let fx = VarRows::new(tall_ids(300));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        // Jump deep into never-measured territory to fling back up through.
        root.event(&mut (), &wheel(8_000.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);

        let trace = fling_up_and_pump(&mut root, &mut logic, &fx, 200.0);
        assert_monotonic_upward(&trace);
        assert!(
            trace.iter().any(|&(_, _, pending)| pending > 0.0),
            "the fling crossed rows measuring taller than the estimate"
        );
        let (offset_at_rest, _, owed) = *trace.last().expect("the fling stopped");
        assert!(
            owed > 0.0,
            "the whole accumulated sum is still owed when the fling stops"
        );

        // The first rebuild after it stops commits the sum — invisibly.
        let w = list_widget(&root);
        let (anchor_id, anchor_y) = anchor_row(w, &fx);
        let (flags, _) = var_frame(&mut root, &mut logic, &fx, 6_000.0);
        let w = list_widget(&root);
        assert!(
            (w.offset() - (offset_at_rest + owed)).abs() < 1e-9,
            "settling committed the accumulated correction in full"
        );
        let (id_after, y_after) = anchor_row(w, &fx);
        assert_eq!(id_after, anchor_id, "the same row is still at the top");
        assert!(
            (y_after - anchor_y).abs() < 1e-9,
            "committing an accumulated correction is invisible ({anchor_y} -> {y_after})"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));

        // ...and the list comes to rest instead of oscillating.
        let mut settled = None;
        for k in 0..6 {
            let before = list_widget(&root).offset();
            var_frame(&mut root, &mut logic, &fx, 6_016.0 + 16.0 * k as f64);
            let w = list_widget(&root);
            if w.pending_correction == 0.0 && w.offset() == before {
                settled = Some(before);
                break;
            }
        }
        let settled = settled.expect("the list settles after the fling");
        for k in 0..3 {
            var_frame(&mut root, &mut logic, &fx, 6_112.0 + 16.0 * k as f64);
            assert_eq!(
                list_widget(&root).offset(),
                settled,
                "the offset is at rest, not oscillating"
            );
        }
    }

    #[test]
    fn an_upward_fling_over_shorter_rows_accumulates_the_other_way() {
        // The mirror: rows measure 40 against the same 60px estimate, so the
        // withheld sum is negative — the direction that, applied per frame,
        // over-travels the fling rather than reversing it. Same contract.
        let fx = VarRows::new(short_ids(400));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(8_000.0));
        settle(&mut root, &mut logic, &fx, 100.0, 4);

        let trace = fling_up_and_pump(&mut root, &mut logic, &fx, 200.0);
        assert_monotonic_upward(&trace);
        let (offset_at_rest, _, owed) = *trace.last().expect("the fling stopped");
        assert!(owed < 0.0, "shorter-than-estimated rows owe a negative sum");

        let w = list_widget(&root);
        let (anchor_id, anchor_y) = anchor_row(w, &fx);
        var_frame(&mut root, &mut logic, &fx, 6_000.0);
        let w = list_widget(&root);
        assert!((w.offset() - (offset_at_rest + owed)).abs() < 1e-9);
        let (id_after, y_after) = anchor_row(w, &fx);
        assert_eq!(id_after, anchor_id);
        assert!(
            (y_after - anchor_y).abs() < 1e-9,
            "committing an accumulated correction is invisible ({anchor_y} -> {y_after})"
        );
    }

    #[test]
    fn layout_and_paint_never_move_the_offset_while_a_correction_is_pending() {
        // Criterion 4, pass by pass: layout may measure a correction but never
        // apply one, paint touches the offset only through the fling pump (inert
        // here), and rebuild is the one pass that commits.
        let fx = VarRows::new(tall_ids(200));
        let mut logic = fx.logic();
        let mut root = converged_var(&mut logic, &fx);

        root.event(&mut (), &wheel(1_500.0));
        fx.probe.pass.set(Pass::Rebuild);
        root.rebuild(&mut logic, &mut ());
        let after_rebuild = list_widget(&root).offset();

        fx.probe.pass.set(Pass::Layout);
        root.layout(KEYED_WINDOW);
        let w = list_widget(&root);
        assert!(
            w.pending_correction != 0.0,
            "layout measured a correction to record"
        );
        assert_eq!(
            w.offset(),
            after_rebuild,
            "...and did not apply it to the offset"
        );

        fx.probe.pass.set(Pass::Paint);
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::from_nanos(100_000_000));
        fx.probe.pass.set(Pass::Idle);
        let w = list_widget(&root);
        assert!(
            !w.is_flinging(),
            "no fling is in flight, so the pump is inert"
        );
        assert_eq!(
            w.offset(),
            after_rebuild,
            "paint moves the offset only via the fling pump"
        );
        assert!(w.pending_correction != 0.0, "the correction is still owed");
        assert!(
            outcome.needs_frame,
            "and paint asked for the frame that commits it"
        );

        let (flags, _) = var_frame(&mut root, &mut logic, &fx, 116.0);
        let w = list_widget(&root);
        assert_eq!(w.pending_correction, 0.0);
        assert!(
            w.offset() > after_rebuild,
            "rebuild is the pass that commits it"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    #[test]
    fn uniform_and_positional_modes_never_carry_a_correction() {
        // Criterion 5: this is variable-extent-only machinery. A uniform keyed
        // list and a positional list measure nothing, so neither can ever owe a
        // correction — and their placement is literally their offset.
        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut keyed = fx.keyed_logic();
        let mut root = converged(&mut keyed, &fx.log);
        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut keyed, &fx.log, 32.0);
        fx.rows.borrow_mut().splice(0..0, [9990, 9980]);
        keyed_frame(&mut root, &mut keyed, &fx.log, 48.0);
        let w = list_widget(&root);
        assert_eq!(
            w.pending_correction, 0.0,
            "a uniform keyed list measures nothing to correct from"
        );
        assert_eq!(w.placement_offset(), w.offset());

        let fx = KeyedRows::new((0..1000).map(|i| i as u64 * 10).collect());
        let mut positional = fx.positional_logic();
        let mut root = converged(&mut positional, &fx.log);
        root.event(&mut (), &wheel(500.0));
        keyed_frame(&mut root, &mut positional, &fx.log, 32.0);
        fx.rows.borrow_mut().insert(0, 12_345);
        keyed_frame(&mut root, &mut positional, &fx.log, 48.0);
        let w = list_widget(&root);
        assert_eq!(
            w.pending_correction, 0.0,
            "a positional list measures nothing to correct from"
        );
        assert_eq!(w.placement_offset(), w.offset());
    }

    /// A hand-built variable-extent widget carrying an uncommitted correction and
    /// a near-start callback — the deterministic shape of "an uncommitted
    /// correction decides no edge" (the [`near_start_widget`] idiom).
    fn pending_correction_widget(pending: f64) -> ListViewWidget {
        let mut w = ListViewWidget::new(1000, ESTIMATE);
        w.key_of = Some(Rc::new(|i: usize| ChildKey::new(i as u64)));
        w.estimated_extent = Some(ESTIMATE);
        w.viewport = Size::new(200.0, 200.0);
        w.near_start_threshold = 100.0;
        let cb: Rc<dyn Fn(&mut Loads)> = Rc::new(|s: &mut Loads| s.count += 1);
        w.on_near_start = Some(crate::authoring::erase_callback(&cb));
        w.near_start_armed = true;
        // The raw offset sits inside the near-start threshold; the content it
        // describes does not.
        w.offset = 50.0;
        w.pending_correction = pending;
        w
    }

    #[test]
    fn an_uncommitted_correction_decides_no_edge() {
        // The edge-latch half: edges read the placement, so an uncommitted
        // correction neither fires one on the raw offset's say-so nor re-fires
        // one when it commits (the placement is exactly what a commit leaves
        // unchanged).
        let mut w = pending_correction_widget(500.0);
        let mut state = Loads::default();
        run_loads(&mut w, &mut state, &wheel(0.0));
        assert_eq!(
            state.count, 0,
            "at a placement of 550 the list is nowhere near the start"
        );

        assert!(w.apply_pending_correction(), "the commit applies");
        assert_eq!(w.offset(), 550.0);
        assert_eq!(w.pending_correction, 0.0);
        run_loads(&mut w, &mut state, &wheel(0.0));
        assert_eq!(
            state.count, 0,
            "committing a correction is bookkeeping, not scroll motion"
        );

        // A genuine approach still fires it, exactly once.
        run_loads(&mut w, &mut state, &wheel(-500.0));
        assert_eq!(w.offset(), 50.0);
        assert_eq!(state.count, 1, "a real approach fires the load-older edge");
        run_loads(&mut w, &mut state, &wheel(-20.0));
        assert_eq!(state.count, 1, "and it stays edge-triggered");
    }

    #[test]
    fn a_live_fling_withholds_the_correction_until_it_stops() {
        // The accumulate-then-apply rule at its own granularity, over both ways
        // a fling ends: decaying under FLING_STOP, and a pointer taking over.
        let mut w = pending_correction_widget(500.0);
        w.fling = Some(-40.0);
        assert!(
            !w.apply_pending_correction(),
            "a live fling withholds the commit"
        );
        assert_eq!(w.pending_correction, 500.0, "the sum keeps accumulating");
        for _ in 0..12 {
            w.tick(16.0);
        }
        assert!(!w.is_flinging(), "the fling decayed under FLING_STOP");
        assert!(w.apply_pending_correction(), "and the next rebuild commits");
        assert_eq!(w.pending_correction, 0.0);

        // Pointer-down takeover: same commit, one event earlier.
        let mut w = pending_correction_widget(500.0);
        let mut state = Loads::default();
        w.fling = Some(-4_000.0);
        assert!(!w.apply_pending_correction());
        run_loads(&mut w, &mut state, &ev(PointerPhase::Down, 50.0));
        assert!(!w.is_flinging(), "a pointer down takes the gesture over");
        assert!(w.apply_pending_correction());
        assert_eq!(w.pending_correction, 0.0);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "duplicate key")]
    fn duplicate_keys_trip_the_debug_assert_on_build() {
        let mut logic = |_: &mut ()| {
            ListView::builder_keyed(
                5,
                ROW_EXTENT,
                |_| ChildKey::new(7u32),
                |i| any::<(), _>(gen_stub(i)),
            )
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        frame(&mut root, &mut logic, &mut (), KEYED_WINDOW, 0.0);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "duplicate key")]
    fn duplicate_keys_trip_the_debug_assert_on_rebuild() {
        let collide = Rc::new(Cell::new(false));
        let collide_l = collide.clone();
        let mut logic = move |_: &mut ()| {
            let collide = collide_l.clone();
            ListView::builder_keyed(
                5,
                ROW_EXTENT,
                move |i| ChildKey::new(if collide.get() { 7 } else { i }),
                |i| any::<(), _>(gen_stub(i)),
            )
        };
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        frame(&mut root, &mut logic, &mut (), KEYED_WINDOW, 0.0);
        collide.set(true);
        frame(&mut root, &mut logic, &mut (), KEYED_WINDOW, 16.0);
    }

    // --- (13) The M3E stretch effect, the twin of `scroll.rs`'s own group:
    //      rows stay where the windowing offset puts them and the pull is
    //      painted as an affine about the held edge instead. ---

    /// [`rubber_band_logic`] with [`OverscrollEffect::Stretch`] selected on the
    /// view, so the effect reaches the widget through the real build/rebuild
    /// path rather than being poked onto the element. Pinned to `RubberBand`
    /// like the rest of the feel fixtures — the pull *values* the stretch is
    /// read from are this physics' (the effect itself is physics-agnostic, and
    /// `stretch_under_boundary_rejection_uses_edge_pull` covers the clamping
    /// pairing the Android default ships).
    fn stretch_logic(item_count: usize) -> impl FnMut(&mut ()) -> ListView<()> {
        move |_: &mut ()| {
            let mut view = list_view(item_count, 50.0, |i| any::<(), _>(gen_stub(i)))
                .physics(RubberBand::new());
            view.effect = OverscrollEffect::Stretch;
            view
        }
    }

    /// Drive the shared past-top drag: `Down`, a 40px past-slop takeover, then
    /// 20px further past the already-at-top edge (`scroll.rs`'s fixture
    /// gesture), painting a frame between each so the tree stays live.
    fn drag_20px_past_top(
        root: &mut RenderRoot<(), ListView<()>>,
        logic: &mut impl FnMut(&mut ()) -> ListView<()>,
        state: &mut (),
        window: Size,
    ) {
        frame(root, logic, state, window, 0.0);
        frame(root, logic, state, window, 16.0);
        root.event(state, &ev(PointerPhase::Down, 50.0));
        frame(root, logic, state, window, 32.0);
        root.event(state, &ev(PointerPhase::Move, 90.0)); // 40px > slop → takeover
        frame(root, logic, state, window, 48.0);
        root.event(state, &ev(PointerPhase::Move, 110.0));
    }

    /// Paint the live tree into a recording scene and hand back the transforms
    /// the stretch pushed (mirrors `scroll.rs`'s `painted_stretch` observable).
    fn painted_transforms(root: &mut RenderRoot<(), ListView<()>>, ms: f64) -> Vec<kurbo::Affine> {
        let mut scene = crate::test_support::RecordingScene::default();
        root.paint(&mut scene, FrameTime::from_nanos((ms * 1_000_000.0) as u64));
        assert_eq!(
            scene.transforms.len(),
            scene.transform_pops as usize,
            "every pushed transform must be popped in the same paint"
        );
        scene.transforms
    }

    #[test]
    fn stretch_keeps_row_origins_fixed() {
        let window = Size::new(200.0, 200.0);

        // The same drag under each effect: the physics' answer is identical,
        // only what paint does with it differs.
        let mut translated: RenderRoot<(), ListView<()>> = RenderRoot::new();
        drag_20px_past_top(
            &mut translated,
            &mut rubber_band_logic(1000),
            &mut (),
            window,
        );
        let mut stretched: RenderRoot<(), ListView<()>> = RenderRoot::new();
        drag_20px_past_top(&mut stretched, &mut stretch_logic(1000), &mut (), window);

        let translate = list_widget(&translated);
        let stretch = list_widget(&stretched);
        assert_eq!(stretch.offset(), 0.0, "the windowing offset never moves");
        assert_eq!(
            stretch.overscroll, -10.0,
            "the resisted displacement is the physics' answer, effect or not"
        );
        assert_eq!(translate.overscroll, stretch.overscroll);
        assert_eq!(translate.edge_pull, stretch.edge_pull);

        // Row 0 sits at content y = 0: Translate paints it 10px down (the
        // displacement), Stretch leaves it at the viewport's top edge.
        assert_eq!(translate.children[0].origin().y, 10.0);
        assert_eq!(stretch.children[0].origin().y, 0.0);
        assert_eq!(
            translate.children[0].origin().y - stretch.children[0].origin().y,
            -stretch.overscroll,
            "the two fixtures differ by exactly the overscroll displacement"
        );

        // …and only the stretched one paints a transform for the pull, about
        // the pulled (top) edge: `[1, 0, 0, s, 0, anchor·(1 − s)]`.
        assert!(painted_transforms(&mut translated, 64.0).is_empty());
        let transforms = painted_transforms(&mut stretched, 64.0);
        assert_eq!(
            transforms.len(),
            1,
            "one stretch transform for the viewport"
        );
        let [a, b, c, d, e, f] = transforms[0].as_coeffs();
        assert_eq!([a, b, c, e], [1.0, 0.0, 0.0, 0.0], "scroll-axis-only scale");
        assert!(
            (d - (1.0 + crate::scroll::stretch_intensity(-10.0, 200.0))).abs() < 1e-12,
            "scale is 1 + the shared curve's intensity: {d}"
        );
        assert!(
            f.abs() < 1e-9,
            "a top pull scales about the viewport's top edge: {f}"
        );
    }

    #[test]
    fn stretch_settles_back_to_identity() {
        let window = Size::new(200.0, 200.0);
        let mut logic = stretch_logic(1000);
        let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
        let mut state = ();
        drag_20px_past_top(&mut root, &mut logic, &mut state, window);
        assert_eq!(painted_transforms(&mut root, 64.0).len(), 1);

        // Release under the refresh trigger → the settle decays the pull, and
        // the stretch with it (no `request_layout` anywhere: the pump's own
        // continuation frames are what keep it animating).
        root.event(&mut state, &ev(PointerPhase::Up, 110.0));
        assert!(list_widget(&root).settling);
        let mut ms = 80.0;
        for _ in 0..30 {
            frame(&mut root, &mut logic, &mut state, window, ms);
            ms += 16.0;
            if !list_widget(&root).settling {
                break;
            }
        }
        let w = list_widget(&root);
        assert!(!w.settling, "the settle terminated");
        assert_eq!(w.edge_pull, 0.0, "a completed settle leaves no pull");
        assert_eq!(w.overscroll, 0.0);
        assert_eq!(w.children[0].origin().y, 0.0, "the rows never moved");
        assert!(
            painted_transforms(&mut root, ms).is_empty(),
            "…so paint pushes no transform at all — back to identity"
        );
    }

    /// The one transform a bare fixture's own `paint` pushes for the stretch,
    /// as `(anchor_y, scale_y)` off the
    /// `translate(anchor)·scale(1, s)·translate(−anchor)` sandwich — the twin
    /// of `scroll.rs`'s `painted_stretch`, over a widget with no materialized
    /// rows so the stretch is the whole scene. `None` when paint pushed none.
    ///
    /// **At rest only**: `paint` pumps the animation clock, so calling this
    /// mid-flight would reseed it against this context's zero frame time.
    fn painted_stretch_at_rest(w: &mut ListViewWidget) -> Option<(f64, f64)> {
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        let mut scene = crate::test_support::RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(
            scene.transforms.len(),
            scene.transform_pops as usize,
            "every pushed transform must be popped in the same paint"
        );
        let [a, b, c, d, e, f] = scene.transforms.first()?.as_coeffs();
        assert_eq!(
            [a, b, c, e],
            [1.0, 0.0, 0.0, 0.0],
            "a scroll-axis-only scale: no x scale, no skew, no x translation"
        );
        Some((f / (1.0 - d), d))
    }

    /// The `ScrollView` twin of this test lives in `scroll.rs`, as
    /// `clamping_fling_into_the_edge_settles_the_stretch`.
    #[test]
    fn clamping_fling_into_the_edge_settles_the_stretch() {
        use crate::physics::Tolerance;
        use crate::physics::simulation::ClampingScrollSimulation;

        // The shipped Android pairing, run on the host: parked 100px short of
        // the bottom, released at 1000 px/s straight into it. The clamping
        // curve is unbounded, so the driver pins the offset and routes the
        // whole overshoot into `edge_pull` — the release must not leave that
        // pull, and the stretch it paints, standing. A bare fixture (no rows
        // materialized): this pins the release/settle arithmetic, not
        // windowing.
        let mut w = ListViewWidget::new(1000, 50.0);
        w.viewport = Size::new(200.0, 200.0);
        w.physics = Rc::new(Clamping::new());
        w.effect = OverscrollEffect::Stretch;
        let bottom = w.max_offset();
        dispatch_list(&mut w, &wheel(bottom - 100.0), 0.0);
        assert_eq!(w.offset(), bottom - 100.0, "parked 100px short of the end");

        dispatch_list(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch_list(&mut w, &ev(PointerPhase::Move, 68.0), 16.0); // 32px > slop
        dispatch_list(&mut w, &ev(PointerPhase::Move, 68.0), 32.0);
        dispatch_list(&mut w, &ev(PointerPhase::Up, 68.0), 32.0);
        assert_close(release_velocity(&w), 1000.0, 1e-9, "the release velocity");
        assert!(w.ballistic.is_some(), "…as a real ballistic curve");

        let curve = ClampingScrollSimulation::new(
            bottom - 100.0,
            1000.0,
            ClampingScrollSimulation::DEFAULT_FRICTION,
            Tolerance::for_device_pixel_ratio(METRICS_FALLBACK_DPR),
        );
        assert!(
            curve.final_x() > bottom + 100.0,
            "the spline must overshoot the extent by >100px: {}",
            curve.final_x()
        );
        let spline_frames = (curve.duration() * 1000.0 / 16.0).ceil();

        let mut ms = 100.0;
        let mut frames = 0.0;
        let mut ballistic_frames = 0.0;
        let mut peak = 0.0f64;
        loop {
            let mut ctx = PaintCtx::for_test(
                Point::ZERO,
                w.viewport,
                FrameTime::from_nanos((ms * 1_000_000.0) as u64),
            );
            w.pump_fling(&mut ctx);
            peak = peak.max(w.edge_pull.abs());
            if w.ballistic.is_some() {
                ballistic_frames += 1.0;
            }
            ms += 16.0;
            frames += 1.0;
            assert!(
                frames < 2_000.0,
                "the release never came to rest: edge_pull {}",
                w.edge_pull
            );
            if !ctx.needs_frame() {
                break;
            }
        }

        assert!(
            peak > 1.0,
            "the fling must actually reach the edge for this to mean anything: {peak}"
        );
        assert_close(
            w.offset(),
            bottom,
            1e-9,
            "the offset ends pinned at the end",
        );
        assert_eq!(
            w.overscroll, 0.0,
            "the windowing offset carries no leftover"
        );
        assert_eq!(w.edge_pull, 0.0, "a finished fling leaves no pull standing");
        assert_eq!(
            painted_stretch_at_rest(&mut w),
            None,
            "…so paint pushes no transform at all — back to identity"
        );
        assert!(
            ballistic_frames < spline_frames / 2.0,
            "the pinned curve ran {ballistic_frames} frames of a {spline_frames}-frame spline"
        );
    }
}
