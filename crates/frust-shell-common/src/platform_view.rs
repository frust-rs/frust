//! Platform-agnostic native-sibling compositor logic (task 03): turns the raw,
//! per-paint-pass [`PlatformViewFrame`] collection (`frust-core`, task 01) into
//! an idempotent, generation-stamped command list — [`ViewCommand`] — both
//! mobile shells' FFI peek getters serve to their platform side (tasks 05/06).
//!
//! # Why this lives in `frust-shell-common`
//!
//! This is pure diffing logic with no FFI, no JSON, and no platform types — the
//! same "platform-agnostic brain, shell-owned wire format" split this crate
//! already draws elsewhere (`frame_gate`'s skip decision, `resample`'s pointer
//! interpolation). JSON encoding of a [`ViewCommand`] batch stays hand-rolled in
//! each shell's own FFI glue (`docs/CODE_STANDARDS.md`'s "hand-roll JSON at the
//! mobile FFI boundary" rule) — this module never touches `serde` or any string
//! wire format, only the typed command vocabulary.
//!
//! # The `frust-core` → differ contract
//!
//! `frust-core::app::RenderRoot::platform_view_frames()` replaces its whole
//! `Vec<PlatformViewFrame>` every paint pass and stays deliberately dumb: a
//! slot absent from one pass's frames might be culled-but-still-alive,
//! momentarily not repainting, or genuinely torn down — core has no teardown
//! hook to tell those apart (task 01's notes). [`PlatformViewState`] is where
//! that ambiguity gets resolved, by watching how long a slot stays missing
//! (see `missing_streak` below).
//!
//! # Command semantics
//!
//! - **New `slot_id`** ⇒ [`ViewCommand::Create`] then [`ViewCommand::Update`]
//!   in the same ingest batch, in that order — the native side never sees an
//!   `Update` for a view it hasn't been told to create yet.
//! - **Rect/clip/visible change past [`EPSILON_PX`]** ⇒ `Update`; a change
//!   smaller than that (or no change at all) emits nothing — a no-op poll is
//!   free, so a shell can call [`PlatformViewState::commands`] every frame
//!   with no cost when nothing moved.
//! - **`params_json` change** (detected via `params_generation`, bumped by the
//!   widget — task 02 — whenever it edits `params_json`) ⇒
//!   [`ViewCommand::UpdateParams`], independent of the rect/clip/visible
//!   comparison above.
//! - **`view_type` change** on a live slot ⇒ [`ViewCommand::Dispose`] followed
//!   by a fresh `Create` + `Update`, **in the same ingest**. A different
//!   `view_type` is a different native factory, so the old view cannot be
//!   re-parameterized into the new one; emitting only a `Create` would be
//!   ignored by a host that already has a view for that slot id, and waiting
//!   out the [`DISPOSE_AFTER_MISSING_FRAMES`] streak would never happen at all
//!   (the slot is still present every pass). See the D-4 note in
//!   [`PlatformViewState::ingest`].
//! - **Missing for [`HIDE_AFTER_MISSING_FRAMES`] consecutive ingests** (while
//!   the slot was last visible) ⇒ `Update { visible: false }` — a Hide. Only
//!   fires once per hide (the slot's tracked `last_visible` flips to `false`,
//!   so the same missing streak never re-emits it).
//! - **Missing for [`DISPOSE_AFTER_MISSING_FRAMES`] consecutive ingests** ⇒
//!   [`ViewCommand::Dispose`], and the slot is forgotten — a later
//!   reappearance of the same `slot_id` is indistinguishable from a brand-new
//!   one and gets a fresh `Create` (idempotent either way — see module docs'
//!   Widget teardown tradeoff below). [`PlatformViewState::retire`] is the
//!   second, explicit path to the same outcome, for a shell that has (or later
//!   gains) a real teardown signal — both paths are kept deliberately, per the
//!   plan.
//! - **Revive after Hide** (slot reappears in `ingest`'s frames before the
//!   dispose threshold): since the slot is still tracked, this is just an
//!   ordinary `Update` — `visible` flips back to `true` like any other
//!   changed field, no `Create`.
//! - **Revive after Dispose**: the slot was forgotten, so this is
//!   indistinguishable from new — fresh `Create` + `Update`.
//!
//! # Widget teardown detection tradeoff
//!
//! `frust-core` has no teardown hook in the frame channel (task 01 keeps core
//! "dumb" deliberately), so [`PlatformViewState`] cannot know for certain that
//! a missing slot's widget was actually dropped from the tree versus merely
//! culled or transiently not repainting. [`DISPOSE_AFTER_MISSING_FRAMES`] is a
//! heuristic streak threshold, not a real signal — a shell that gains an
//! actual teardown channel in the future should call [`PlatformViewState::retire`]
//! directly instead of waiting out the streak; both paths converge on the same
//! `Dispose` command and the same "next Create is fresh" semantics, so neither
//! needs to be removed once the other exists.
//!
//! # Generation / acknowledgement / compaction
//!
//! [`PlatformViewState::commands`] returns `(generation, &[ViewCommand])` — the
//! **entire** not-yet-acknowledged command backlog, not just the latest
//! batch. `generation` only advances when [`PlatformViewState::ingest`] (or
//! [`PlatformViewState::reset_for_surface_recreate`]/[`PlatformViewState::retire`])
//! actually produces at least one command; a no-change ingest leaves it
//! untouched, so a shell polling every frame can cheaply tell "nothing new"
//! apart from "here's more to apply" without diffing the slice itself.
//! [`PlatformViewState::acknowledge`] tells the state that the native side has
//! finished applying everything up through a given generation, letting it
//! **compact** (drop) those entries from the backlog — this is what makes a
//! missed poll during surface recreation safe: the native side just re-polls
//! [`commands`](PlatformViewState::commands) and gets the same backlog again
//! (nothing was dropped until acknowledged), and re-applying an already-applied
//! prefix is safe because the command stream is a replay of state transitions,
//! not one-shot deltas.
//!
//! # Backlog cap
//!
//! The backlog only shrinks on [`acknowledge`](PlatformViewState::acknowledge),
//! so a native side that stops acking (a wedged host, a lost view hierarchy)
//! would otherwise grow it for the process lifetime — and a camera preview is
//! the first genuinely long-lived slot, so "the app exits before it matters" is
//! no longer an answer. Past [`MAX_PENDING_COMMANDS`] entries the backlog is
//! **compacted into its own net effect**: one `Dispose` per slot the dropped
//! entries tore down, then a full `Create` + `Update` replay of every live slot
//! — exactly the surface-recreate replay
//! ([`reset_for_surface_recreate`](PlatformViewState::reset_for_surface_recreate)),
//! which is already the established "the native side must rebuild from this
//! alone" batch. Every dropped intermediate is a state transition the replay
//! supersedes, so a native side that applies only the compacted batch lands in
//! the same place. The whole compacted batch carries the current generation, so
//! an ack of an older one drops none of it.
//!
//! # Frame pairing (the release gate)
//!
//! [`commands`](PlatformViewState::commands) hands the native side the whole
//! backlog the instant it exists — which is *earlier* than the frust frame that
//! produced the geometry reaches the screen, so a scrolling hosted view runs
//! visibly ahead of the frust content it is supposed to be pinned to
//! (`workflow/plans/features/frust-camera/research/SPIKE-SYNC.md` §1). A shell
//! that knows which frust frame each batch came from closes that gap by
//! releasing only the prefix whose frame is already presented:
//! [`FramePairing`] keeps the `(generation, frame_id)` bookkeeping and
//! [`commands_up_to`](PlatformViewState::commands_up_to) serves the prefix.
//!
//! # Skip-safety
//!
//! A gate-skipped frame (`docs/ARCHITECTURE.md`'s Frame gate) calls nothing —
//! a shell simply never calls [`PlatformViewState::ingest`] on a `Skip`
//! decision, so no rect can appear to "move" during a skip (paint doesn't run,
//! so `PaintCtx::visible_rect`/scroll state can't have changed either) —
//! nothing in this module special-cases a skip; the contract is entirely
//! "don't call ingest".

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use frust_core::widget::PlatformViewFrame;
use kurbo::Rect;

/// Below this many logical px of difference on every edge, a rect/clip change
/// is not worth an [`ViewCommand::Update`] — see the module docs' Command
/// semantics section. Chosen to absorb floating-point layout jitter (e.g. a
/// scroll offset accumulating sub-pixel drift) without visibly lagging a
/// genuinely moving native sibling view.
pub const EPSILON_PX: f64 = 0.5;

/// Consecutive `ingest` calls a previously-live, previously-visible slot may
/// be absent from `frames` before it is Hidden (`Update { visible: false }`).
/// See the module docs' Command semantics section.
pub const HIDE_AFTER_MISSING_FRAMES: u32 = 2;

/// Consecutive `ingest` calls a slot may be absent from `frames` before it is
/// Disposed outright. A heuristic streak, not a real teardown signal — see
/// the module docs' Widget teardown detection tradeoff.
pub const DISPOSE_AFTER_MISSING_FRAMES: u32 = 30;

/// Upper bound on the not-yet-acknowledged command backlog before it is
/// compacted into its own net effect — see the module docs' Backlog cap.
///
/// Sized to be unreachable in normal operation (a steadily-acking native side
/// keeps the backlog at one frame's worth, single digits) while still bounding
/// a stuck one: the compaction itself costs `2 × live slots` commands, so the
/// cap only has to sit comfortably above that for any realistic slot count.
pub const MAX_PENDING_COMMANDS: usize = 256;

/// How far a batch's frust frame may fall behind the submission cursor before
/// [`FramePairing`] releases the batch anyway — the release gate's staleness
/// escape hatch. **Required, not defensive**: the UI→render scene channel is
/// depth-1 latest-wins, so a scene the UI thread submitted may be overtaken and
/// never rendered at all, and a dropped frame's id never presents. Without this
/// arm one dropped scene strands every later batch forever (SPIKE-SYNC §2.5's
/// lesson: a submission counter is not a presented counter).
///
/// Measured pipeline depth on the two Phase-0 test devices was 2–4 frames, so
/// 12 sits well above the working range while bounding worst-case staleness to
/// ~100 ms at 120 Hz.
pub const MAX_FRAMES_IN_FLIGHT: u64 = 12;

/// One native-sibling-compositor instruction — the differ's whole output
/// vocabulary. `Clone + PartialEq + Debug` (per the task notes) so a golden
/// test can assert an exact command sequence.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewCommand {
    /// Create a new native view for `slot_id`. Always immediately followed,
    /// in the same batch, by an [`ViewCommand::Update`] placing it.
    Create {
        /// Stable per-widget-instance id (see `frust_core::widget::next_slot_id`).
        slot_id: u64,
        /// `"dev.frust.<Factory>"` view-factory identifier.
        view_type: String,
        /// Opaque creation params for the native factory (may be empty).
        params_json: String,
    },
    /// Place/resize/clip/show-or-hide an already-created slot. Logical px,
    /// absolute window coordinates (mirroring [`PlatformViewFrame`]) — the
    /// receiving shell scales to physical px at its own FFI boundary.
    Update {
        /// Which slot this applies to.
        slot_id: u64,
        /// Absolute paint bounds.
        rect: Rect,
        /// Visible-rect intersection, or `None` when fully visible.
        clip: Option<Rect>,
        /// `false` ⇒ hide the native view without disposing it.
        visible: bool,
    },
    /// `params_json` changed (`params_generation` advanced) with no
    /// necessary rect/clip/visible change — a separate command so a shell
    /// doesn't have to re-place a view just to hand it new creation params.
    UpdateParams {
        /// Which slot this applies to.
        slot_id: u64,
        /// The new opaque params payload.
        params_json: String,
    },
    /// Tear down a slot's native view entirely. A `slot_id` reused after this
    /// (the same numeric id reappearing in a later `ingest`) is treated as
    /// brand-new — see the module docs' Widget teardown detection tradeoff.
    Dispose {
        /// Which slot to tear down.
        slot_id: u64,
    },
}

/// Per-slot last-emitted state the differ compares each `ingest` call
/// against, to decide whether anything actually changed.
#[derive(Clone, Debug)]
struct SlotEntry {
    view_type: String,
    params_json: String,
    params_generation: u64,
    last_rect: Rect,
    last_clip: Option<Rect>,
    last_visible: bool,
    /// Consecutive `ingest` calls this slot has been absent from `frames`.
    /// Reset to `0` the instant it reappears.
    missing_streak: u32,
}

/// The differ: per-slot last-seen state plus the accumulated, not-yet-acknowledged
/// [`ViewCommand`] backlog. See the module docs for the full semantics.
///
/// `live` is a [`BTreeMap`] (keyed by `slot_id`), not a `HashMap` — iteration
/// order must be deterministic (ascending `slot_id`) for the "same ingest
/// sequence ⇒ identical command stream" golden-test guarantee (acceptance
/// criterion 3); a frame's own `Create`+`Update` ordering is separately
/// guaranteed by iterating `frames` itself in the caller's given order.
#[derive(Debug, Default)]
pub struct PlatformViewState {
    live: BTreeMap<u64, SlotEntry>,
    /// Flat, contiguous command backlog — kept flat (rather than one `Vec`
    /// per generation) so [`commands`](Self::commands) can return a zero-copy
    /// `&[ViewCommand]` slice.
    pending: Vec<ViewCommand>,
    /// Parallel to `pending`: the generation each entry was pushed under.
    /// Monotonically non-decreasing (generations only ever go up), which is
    /// what lets [`acknowledge`](Self::acknowledge) binary-search the
    /// compaction boundary.
    pending_gens: Vec<u64>,
    generation: u64,
    acked_generation: u64,
    /// Whether the backlog-cap overflow has already been logged — once per
    /// state, so a permanently-unacking native side costs one log line, not one
    /// per compaction (module docs' Backlog cap).
    overflow_logged: bool,
}

impl PlatformViewState {
    /// A fresh differ with no live slots and generation `0`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one paint pass's frames (`RenderRoot::platform_view_frames()`).
    /// Returns `true` if this call produced at least one command (i.e. the
    /// generation advanced) — a caller that only cares "did anything change"
    /// can skip calling [`commands`](Self::commands) entirely when this is
    /// `false`.
    ///
    /// Do not call this on a gate-skipped frame — see the module docs'
    /// Skip-safety section.
    pub fn ingest(&mut self, frames: &[PlatformViewFrame]) -> bool {
        let mut batch = Vec::new();
        let mut seen = std::collections::BTreeSet::new();

        for frame in frames {
            seen.insert(frame.slot_id);
            // D-4: the slot's `view_type` changed under a live id. A different
            // `view_type` resolves to a different native factory, so the old
            // view must be torn down and a new one built — in THIS batch. Drop
            // the tracked entry first, so the `None` arm below emits the fresh
            // `Create` + `Update` after the `Dispose`, exactly as it would for
            // an id it had never seen.
            if let Some(entry) = self.live.get(&frame.slot_id)
                && entry.view_type != frame.view_type
            {
                batch.push(ViewCommand::Dispose {
                    slot_id: frame.slot_id,
                });
                self.live.remove(&frame.slot_id);
            }
            match self.live.get_mut(&frame.slot_id) {
                None => {
                    batch.push(ViewCommand::Create {
                        slot_id: frame.slot_id,
                        view_type: frame.view_type.clone(),
                        params_json: frame.params_json.clone(),
                    });
                    batch.push(ViewCommand::Update {
                        slot_id: frame.slot_id,
                        rect: frame.rect,
                        clip: frame.clip,
                        visible: frame.visible,
                    });
                    self.live.insert(
                        frame.slot_id,
                        SlotEntry {
                            view_type: frame.view_type.clone(),
                            params_json: frame.params_json.clone(),
                            params_generation: frame.params_generation,
                            last_rect: frame.rect,
                            last_clip: frame.clip,
                            last_visible: frame.visible,
                            missing_streak: 0,
                        },
                    );
                }
                Some(entry) => {
                    entry.missing_streak = 0;
                    if entry.last_visible != frame.visible
                        || rect_changed(entry.last_rect, frame.rect)
                        || clip_changed(entry.last_clip, frame.clip)
                    {
                        batch.push(ViewCommand::Update {
                            slot_id: frame.slot_id,
                            rect: frame.rect,
                            clip: frame.clip,
                            visible: frame.visible,
                        });
                        entry.last_rect = frame.rect;
                        entry.last_clip = frame.clip;
                        entry.last_visible = frame.visible;
                    }
                    if entry.params_generation != frame.params_generation {
                        batch.push(ViewCommand::UpdateParams {
                            slot_id: frame.slot_id,
                            params_json: frame.params_json.clone(),
                        });
                        entry.params_json = frame.params_json.clone();
                        entry.params_generation = frame.params_generation;
                    }
                }
            }
        }

        // Missing-slot bookkeeping: any previously-live slot absent from this
        // pass's frames. Iterating `self.live` (a BTreeMap) keeps this
        // deterministic across runs.
        let mut disposed = Vec::new();
        for (&slot_id, entry) in self.live.iter_mut() {
            if seen.contains(&slot_id) {
                continue;
            }
            entry.missing_streak += 1;
            if entry.missing_streak == HIDE_AFTER_MISSING_FRAMES && entry.last_visible {
                batch.push(ViewCommand::Update {
                    slot_id,
                    rect: entry.last_rect,
                    clip: entry.last_clip,
                    visible: false,
                });
                entry.last_visible = false;
            }
            if entry.missing_streak >= DISPOSE_AFTER_MISSING_FRAMES {
                batch.push(ViewCommand::Dispose { slot_id });
                disposed.push(slot_id);
            }
        }
        for slot_id in disposed {
            self.live.remove(&slot_id);
        }

        self.push_batch(batch)
    }

    /// Backgrounding path (platform-views tasks 05/06): synthesize
    /// `Update { visible: false }` for every currently-live, currently-visible
    /// slot **immediately**, regardless of its missing streak. A shell calls
    /// this on the platform's backgrounding hook (Android's `onPause`, iOS's
    /// `frust_pause`) — while backgrounded, paint doesn't run, so
    /// [`ingest`](Self::ingest) is never called to drive the ordinary
    /// [`HIDE_AFTER_MISSING_FRAMES`]-streak Hide path; without this explicit
    /// call a backgrounded native sibling view would stay visible (and,
    /// depending on the platform, keep rendering/consuming resources) until the
    /// app resumes and repaints. A slot already hidden (`last_visible ==
    /// false`) emits nothing for it, so calling this on an already-suspended
    /// state (or with no live slots) is a cheap no-op. The `Update` reuses
    /// each slot's last-known rect/clip — no `frames` argument, unlike
    /// [`ingest`](Self::ingest) — since backgrounding doesn't produce a fresh
    /// paint pass to source one from.
    pub fn suspend_all(&mut self) -> bool {
        let mut batch = Vec::new();
        for (&slot_id, entry) in self.live.iter_mut() {
            if entry.last_visible {
                batch.push(ViewCommand::Update {
                    slot_id,
                    rect: entry.last_rect,
                    clip: entry.last_clip,
                    visible: false,
                });
                entry.last_visible = false;
            }
        }
        self.push_batch(batch)
    }

    /// Explicit retire: dispose `slot_id` right now regardless of its missing
    /// streak, for a shell with a real teardown signal (see the module docs'
    /// Widget teardown detection tradeoff). A no-op (returns `false`) if
    /// `slot_id` isn't currently live (already disposed, or never created).
    pub fn retire(&mut self, slot_id: u64) -> bool {
        if self.live.remove(&slot_id).is_some() {
            self.push_batch(vec![ViewCommand::Dispose { slot_id }])
        } else {
            false
        }
    }

    /// Re-emit `Create` + `Update` for every currently-live slot, using each
    /// slot's last-known state (including a currently-hidden slot's
    /// `visible: false`) — the backgrounding/rotation replay a shell calls
    /// when it knows the native side just lost its whole view hierarchy
    /// (surface recreation) and needs every native sibling rebuilt from
    /// scratch, not just the ones that would otherwise change.
    pub fn reset_for_surface_recreate(&mut self) -> bool {
        let mut batch = Vec::new();
        self.replay_live_into(&mut batch);
        self.push_batch(batch)
    }

    /// Append a full `Create` + `Update` replay of every live slot (ascending
    /// `slot_id`, each preserving its last-known rect/clip/visibility) — the
    /// shared body of [`reset_for_surface_recreate`](Self::reset_for_surface_recreate)
    /// and the backlog-cap compaction (module docs' Backlog cap), which are the
    /// same "rebuild everything from this batch alone" statement.
    fn replay_live_into(&self, batch: &mut Vec<ViewCommand>) {
        for (&slot_id, entry) in self.live.iter() {
            batch.push(ViewCommand::Create {
                slot_id,
                view_type: entry.view_type.clone(),
                params_json: entry.params_json.clone(),
            });
            batch.push(ViewCommand::Update {
                slot_id,
                rect: entry.last_rect,
                clip: entry.last_clip,
                visible: entry.last_visible,
            });
        }
    }

    /// Snapshot for a shell's peek getter: the current generation plus the
    /// **entire** not-yet-acknowledged command backlog (not just the latest
    /// ingest's batch) — see the module docs' Generation/acknowledgement
    /// section.
    pub fn commands(&self) -> (u64, &[ViewCommand]) {
        (self.generation, &self.pending)
    }

    /// The prefix of the backlog whose generation is `<= max_generation`, plus
    /// that prefix's own generation — the release-gate half of
    /// [`commands`](Self::commands) (see the module docs' Frame pairing).
    ///
    /// [`commands`](Self::commands) hands over the whole backlog immediately,
    /// which is what makes a hosted view's geometry run ahead of the frust
    /// content it belongs to: the geometry lands in the window's next frame
    /// while the frust buffer it matches is still queued behind the compositor.
    /// A shell that knows which frust frame produced each batch — and which
    /// frames have actually been presented ([`FramePairing`]) — releases only
    /// the batches whose frame is already on screen.
    ///
    /// `pending_gens` is monotonically non-decreasing, so the prefix is a
    /// `partition_point` — the same boundary search
    /// [`acknowledge`](Self::acknowledge) uses. The **reported** generation is
    /// the last released entry's, not `self.generation`, so the native side's
    /// acknowledgement round-trip stays exactly as truthful as before: it only
    /// ever acks what it was actually handed. An empty prefix reports the
    /// already-acknowledged generation, which a shell's FFI glue reads as "no
    /// change" — the same cheap no-op poll as an unchanged frame.
    pub fn commands_up_to(&self, max_generation: u64) -> (u64, &[ViewCommand]) {
        let end = self.pending_gens.partition_point(|&g| g <= max_generation);
        let reported = if end == 0 {
            self.acked_generation
        } else {
            self.pending_gens[end - 1]
        };
        (reported, &self.pending[..end])
    }

    /// The native side has finished applying everything up through
    /// `generation` (an argument the shell round-trips from a prior
    /// [`commands`](Self::commands) call) — compacts the backlog, dropping
    /// every entry whose generation is `<= generation`. Acknowledging a
    /// generation older than (or equal to) one already acknowledged is a
    /// no-op.
    pub fn acknowledge(&mut self, generation: u64) {
        if generation <= self.acked_generation {
            return;
        }
        self.acked_generation = generation;
        let keep_from = self
            .pending_gens
            .partition_point(|&g| g <= self.acked_generation);
        self.pending.drain(0..keep_from);
        self.pending_gens.drain(0..keep_from);
    }

    fn push_batch(&mut self, batch: Vec<ViewCommand>) -> bool {
        if batch.is_empty() {
            return false;
        }
        self.generation += 1;
        let batch_generation = self.generation;
        self.pending_gens
            .extend(std::iter::repeat_n(batch_generation, batch.len()));
        self.pending.extend(batch);
        if self.pending.len() > MAX_PENDING_COMMANDS {
            self.compact_to_net_effect();
        }
        true
    }

    /// Backlog-cap overflow (module docs' Backlog cap): replace the whole
    /// not-yet-acknowledged backlog with the net effect of applying it — one
    /// `Dispose` per slot the backlog tore down (ascending `slot_id`), then a
    /// full replay of every live slot.
    ///
    /// The `Dispose`s must survive: a slot created *before* the un-acked window
    /// and disposed inside it is a native view the host already built and would
    /// otherwise never be told to tear down — a leak. A slot that was disposed
    /// **and** is live again (a `slot_id` reuse, or the D-4 `view_type` swap)
    /// keeps its `Dispose` too, and the replay's `Create` rebuilds it from the
    /// current `view_type`/params — the only ordering that survives a factory
    /// change.
    ///
    /// The whole compacted batch carries the current generation, so an ack of
    /// an older generation drops none of it, and `pending_gens` stays
    /// non-decreasing for both boundary searches.
    fn compact_to_net_effect(&mut self) {
        let disposed: BTreeSet<u64> = self
            .pending
            .iter()
            .filter_map(|cmd| match cmd {
                ViewCommand::Dispose { slot_id } => Some(*slot_id),
                _ => None,
            })
            .collect();
        let mut compacted = Vec::with_capacity(disposed.len() + self.live.len() * 2);
        for slot_id in disposed {
            compacted.push(ViewCommand::Dispose { slot_id });
        }
        self.replay_live_into(&mut compacted);

        if !self.overflow_logged {
            self.overflow_logged = true;
            log::warn!(
                "frust-shell: platform-view command backlog exceeded {MAX_PENDING_COMMANDS} \
                 un-acknowledged entries (is the native side polling?); compacted {} entries \
                 into a {}-command replay. Logged once per process.",
                self.pending.len(),
                compacted.len()
            );
        }

        self.pending_gens.clear();
        self.pending_gens.resize(compacted.len(), self.generation);
        self.pending = compacted;
    }
}

/// The release gate's bookkeeping: which frust frame produced each command
/// batch, and therefore which batches may be handed to the native side yet.
///
/// A shell records `(generation, frame_id)` for every batch
/// [`PlatformViewState::ingest`] produces (the frame that is about to be
/// submitted carries the geometry the batch describes), reads back the id of
/// the last frame the render side actually **presented**, and serves
/// [`PlatformViewState::commands_up_to`] the resulting boundary. Pure logic:
/// no clock, no platform types, no knowledge of how a shell obtains the two
/// cursors — which is what makes the ordering testable on the host, since a
/// mobile shell's own frame loop is not.
///
/// # Why the frame *id*, not a count
///
/// The UI→render scene channel is depth-1 latest-wins, so submissions and
/// presents are not the same clock — under the measured 120 Hz-submit /
/// 60 Hz-present regime they diverge by half the frames. Pairing against a
/// presented *count* over-delays by exactly the dropped frames; pairing against
/// the id of the frame that actually presented does not (SPIKE-SYNC §2.5). The
/// price of the id is that a dropped frame's id never arrives, which is what
/// [`MAX_FRAMES_IN_FLIGHT`] exists for.
///
/// # Lifecycle
///
/// The pairing is a *smoothing* device, not a correctness barrier: a shell
/// [`clear`](Self::clear)s it whenever the frames it refers to stop being
/// meaningful (backgrounding, surface recreation), after which the whole
/// backlog releases immediately — a hide or a full replay must reach the native
/// side even though no further frame will ever present to unlock it.
#[derive(Debug, Default)]
pub struct FramePairing {
    /// `(generation, frame_id)` per produced batch, oldest first. Both
    /// components are monotonically non-decreasing along the queue, which is
    /// what lets [`releasable_generation`](Self::releasable_generation) stop at
    /// the first held entry.
    due: VecDeque<(u64, u64)>,
}

/// Upper bound on tracked-but-unreleased batches. Reached only if the native
/// side stops acking (the same failure mode the backlog cap covers); dropping
/// the **oldest** entry is the safe direction — the boundary search then
/// releases it, and the oldest entry is by construction the one closest to
/// the [`MAX_FRAMES_IN_FLIGHT`] escape hatch anyway.
const MAX_TRACKED_BATCHES: usize = 256;

impl FramePairing {
    /// Fresh, empty pairing — releases everything until a batch is recorded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember that the batch published under `generation` describes geometry
    /// painted by frust frame `frame_id`.
    pub fn record(&mut self, generation: u64, frame_id: u64) {
        if self.due.len() >= MAX_TRACKED_BATCHES {
            self.due.pop_front();
        }
        self.due.push_back((generation, frame_id));
    }

    /// The highest generation releasable right now, given the id of the last
    /// **presented** frame and of the last **submitted** one.
    ///
    /// A batch is releasable once its own frame is on screen, or once that
    /// frame has fallen [`MAX_FRAMES_IN_FLIGHT`] behind the submission cursor
    /// (it was dropped by the latest-wins channel and will never present). The
    /// first batch that is neither caps the boundary at its own generation
    /// minus one, so everything published before it — including a lifecycle
    /// batch that was never paired with a frame at all — still goes out; an
    /// empty queue releases everything.
    pub fn releasable_generation(&self, presented_frame_id: u64, submitted_frame_id: u64) -> u64 {
        for &(generation, due_frame) in &self.due {
            let on_screen = due_frame <= presented_frame_id;
            let stranded = submitted_frame_id.saturating_sub(due_frame) >= MAX_FRAMES_IN_FLIGHT;
            if !on_screen && !stranded {
                return generation.saturating_sub(1);
            }
        }
        u64::MAX
    }

    /// Drop the bookkeeping for every batch the native side has acknowledged —
    /// the same generation the shell hands
    /// [`PlatformViewState::acknowledge`], so the two stay in step.
    pub fn acknowledge(&mut self, generation: u64) {
        while let Some(&(g, _)) = self.due.front() {
            if g <= generation {
                self.due.pop_front();
            } else {
                break;
            }
        }
    }

    /// Forget every pairing (backgrounding, surface recreation) — see the
    /// type's Lifecycle note.
    pub fn clear(&mut self) {
        self.due.clear();
    }

    /// Whether any batch is still waiting to be paired off.
    pub fn is_empty(&self) -> bool {
        self.due.is_empty()
    }
}

fn rect_changed(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() >= EPSILON_PX
        || (a.y0 - b.y0).abs() >= EPSILON_PX
        || (a.x1 - b.x1).abs() >= EPSILON_PX
        || (a.y1 - b.y1).abs() >= EPSILON_PX
}

fn clip_changed(a: Option<Rect>, b: Option<Rect>) -> bool {
    match (a, b) {
        (None, None) => false,
        (Some(a), Some(b)) => rect_changed(a, b),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(slot_id: u64, rect: Rect, visible: bool) -> PlatformViewFrame {
        PlatformViewFrame {
            slot_id,
            view_type: "dev.frust.Test".to_string(),
            params_json: String::new(),
            params_generation: 0,
            rect,
            clip: None,
            visible,
        }
    }

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    #[test]
    fn new_slot_emits_create_then_update_in_order() {
        let mut state = PlatformViewState::new();
        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 100.0, 50.0), true)]);
        assert!(changed);

        let (generation, cmds) = state.commands();
        assert_eq!(generation, 1);
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 100.0, 50.0),
                    clip: None,
                    visible: true,
                },
            ]
        );
    }

    #[test]
    fn sub_epsilon_rect_change_emits_nothing() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 100.0, 50.0), true)]);
        state.acknowledge(1);

        // 0.2px drift on every edge — below the 0.5px epsilon.
        let changed = state.ingest(&[frame(1, r(0.2, 0.2, 100.2, 50.2), true)]);
        assert!(!changed);
        let (generation, cmds) = state.commands();
        assert_eq!(generation, 1); // unchanged — no new batch
        assert!(cmds.is_empty());
    }

    #[test]
    fn past_epsilon_rect_change_emits_update() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 100.0, 50.0), true)]);
        state.acknowledge(1);

        let changed = state.ingest(&[frame(1, r(1.0, 0.0, 100.0, 50.0), true)]);
        assert!(changed);
        let (generation, cmds) = state.commands();
        assert_eq!(generation, 2);
        assert_eq!(
            cmds,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(1.0, 0.0, 100.0, 50.0),
                clip: None,
                visible: true,
            }]
        );
    }

    #[test]
    fn missing_two_consecutive_ingests_hides_a_visible_slot() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(1);

        // Missing once: no hide yet.
        let changed = state.ingest(&[]);
        assert!(!changed);
        assert_eq!(state.commands().1, &[]);

        // Missing twice consecutively: Hide.
        let changed = state.ingest(&[]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: false,
            }]
        );

        // A further missing ingest doesn't re-emit the same Hide.
        state.acknowledge(state.commands().0);
        let changed = state.ingest(&[]);
        assert!(!changed);
    }

    #[test]
    fn missing_past_dispose_threshold_disposes_the_slot() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(1);

        for _ in 0..(DISPOSE_AFTER_MISSING_FRAMES - 1) {
            state.ingest(&[]);
        }
        // Not yet disposed at streak == DISPOSE_AFTER_MISSING_FRAMES - 1.
        let (_, cmds) = state.commands();
        assert!(!cmds.contains(&ViewCommand::Dispose { slot_id: 1 }));

        let changed = state.ingest(&[]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert!(cmds.contains(&ViewCommand::Dispose { slot_id: 1 }));
    }

    #[test]
    fn revive_after_hide_is_a_plain_update_not_a_create() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.ingest(&[]); // streak 1
        state.ingest(&[]); // streak 2: Hide
        state.acknowledge(state.commands().0);

        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: true,
            }]
        );
        assert!(!cmds.iter().any(|c| matches!(c, ViewCommand::Create { .. })));
    }

    #[test]
    fn revive_after_dispose_creates_fresh() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        for _ in 0..DISPOSE_AFTER_MISSING_FRAMES {
            state.ingest(&[]);
        }
        state.acknowledge(state.commands().0);

        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                },
            ]
        );
    }

    #[test]
    fn acknowledge_compacts_the_backlog() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        let (gen1, _) = state.commands();
        state.ingest(&[frame(2, r(0.0, 0.0, 10.0, 10.0), true)]);
        let (gen2, cmds) = state.commands();
        assert_eq!(cmds.len(), 4); // both slots' Create+Update, un-acked

        state.acknowledge(gen1);
        let (generation_after_ack, cmds_after_ack) = state.commands();
        assert_eq!(generation_after_ack, gen2); // generation itself is untouched
        assert_eq!(cmds_after_ack.len(), 2); // only slot 2's batch remains

        // Acknowledging an already-acked (or older) generation is a no-op.
        state.acknowledge(gen1);
        assert_eq!(state.commands().1.len(), 2);
    }

    #[test]
    fn replay_after_reset_reproduces_every_live_slot() {
        let mut state = PlatformViewState::new();
        state.ingest(&[
            frame(1, r(0.0, 0.0, 10.0, 10.0), true),
            frame(2, r(20.0, 0.0, 30.0, 10.0), false),
        ]);
        state.acknowledge(state.commands().0);
        assert_eq!(state.commands().1, &[]);

        let changed = state.reset_for_surface_recreate();
        assert!(changed);
        let (_, cmds) = state.commands();
        // Both slots re-created, each preserving its last-known visibility.
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                },
                ViewCommand::Create {
                    slot_id: 2,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(20.0, 0.0, 30.0, 10.0),
                    clip: None,
                    visible: false,
                },
            ]
        );
    }

    #[test]
    fn two_slot_interleaving_does_not_cross_contaminate() {
        let mut state = PlatformViewState::new();
        // Slot 1 created; slot 2 not yet present.
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(state.commands().0);

        // Slot 2 appears; slot 1 unchanged (no rect/clip/visible drift) — only
        // slot 2's Create+Update should be emitted.
        let changed = state.ingest(&[
            frame(1, r(0.0, 0.0, 10.0, 10.0), true),
            frame(2, r(50.0, 50.0, 60.0, 60.0), true),
        ]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 2,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(50.0, 50.0, 60.0, 60.0),
                    clip: None,
                    visible: true,
                },
            ]
        );
        state.acknowledge(state.commands().0);

        // Now slot 1 moves and slot 2 disappears (one missing frame — not yet
        // hidden): only slot 1's Update should appear.
        let changed = state.ingest(&[frame(1, r(5.0, 0.0, 15.0, 10.0), true)]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(5.0, 0.0, 15.0, 10.0),
                clip: None,
                visible: true,
            }]
        );
    }

    #[test]
    fn params_generation_change_emits_update_params() {
        let mut state = PlatformViewState::new();
        let mut f = frame(1, r(0.0, 0.0, 10.0, 10.0), true);
        f.params_json = "{\"a\":1}".to_string();
        state.ingest(&[f]);
        state.acknowledge(state.commands().0);

        let mut f2 = frame(1, r(0.0, 0.0, 10.0, 10.0), true);
        f2.params_json = "{\"a\":2}".to_string();
        f2.params_generation = 1;
        let changed = state.ingest(&[f2]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[ViewCommand::UpdateParams {
                slot_id: 1,
                params_json: "{\"a\":2}".to_string(),
            }]
        );
    }

    #[test]
    fn suspend_all_hides_every_visible_slot_immediately() {
        let mut state = PlatformViewState::new();
        state.ingest(&[
            frame(1, r(0.0, 0.0, 10.0, 10.0), true),
            frame(2, r(20.0, 0.0, 30.0, 10.0), true),
        ]);
        state.acknowledge(state.commands().0);

        // No missing streak at all — suspend_all fires on the very next call,
        // unlike the ordinary ingest-driven Hide path.
        let changed = state.suspend_all();
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: false,
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(20.0, 0.0, 30.0, 10.0),
                    clip: None,
                    visible: false,
                },
            ]
        );
    }

    #[test]
    fn suspend_all_skips_already_hidden_slots() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), false)]);
        state.acknowledge(state.commands().0);

        // The only live slot is already visible:false — nothing to emit.
        let changed = state.suspend_all();
        assert!(!changed);
        assert_eq!(state.commands().1, &[]);
    }

    #[test]
    fn suspend_all_on_no_live_slots_is_a_noop() {
        let mut state = PlatformViewState::new();
        assert!(!state.suspend_all());
        assert_eq!(state.commands().0, 0);
    }

    #[test]
    fn revive_after_suspend_all_is_a_plain_update() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(state.commands().0);
        state.suspend_all();
        state.acknowledge(state.commands().0);

        // The slot reappears in the next real ingest (e.g. the first frame
        // after resume) — an ordinary Update, no re-Create.
        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: true,
            }]
        );
        assert!(!cmds.iter().any(|c| matches!(c, ViewCommand::Create { .. })));
    }

    #[test]
    fn retire_disposes_immediately_regardless_of_missing_streak() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(state.commands().0);

        assert!(state.retire(1));
        assert_eq!(state.commands().1, &[ViewCommand::Dispose { slot_id: 1 }]);

        // Retiring an already-gone (or never-created) slot is a no-op.
        assert!(!state.retire(1));
        assert!(!state.retire(999));
    }

    // ---- D-4: view_type swap ------------------------------------------------

    #[test]
    fn view_type_swap_disposes_and_recreates_in_the_same_ingest() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(state.commands().0);

        let mut swapped = frame(1, r(0.0, 0.0, 10.0, 10.0), true);
        swapped.view_type = "dev.frust.Other".to_string();
        let changed = state.ingest(&[swapped]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[
                ViewCommand::Dispose { slot_id: 1 },
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Other".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                },
            ]
        );
    }

    #[test]
    fn view_type_swap_does_not_disturb_other_slots() {
        let mut state = PlatformViewState::new();
        state.ingest(&[
            frame(1, r(0.0, 0.0, 10.0, 10.0), true),
            frame(2, r(20.0, 0.0, 30.0, 10.0), true),
        ]);
        state.acknowledge(state.commands().0);

        let mut swapped = frame(2, r(20.0, 0.0, 30.0, 10.0), true);
        swapped.view_type = "dev.frust.Other".to_string();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true), swapped]);
        let (_, cmds) = state.commands();
        assert!(cmds.iter().all(|c| match c {
            ViewCommand::Create { slot_id, .. }
            | ViewCommand::Update { slot_id, .. }
            | ViewCommand::UpdateParams { slot_id, .. }
            | ViewCommand::Dispose { slot_id } => *slot_id == 2,
        }));
    }

    #[test]
    fn unchanged_view_type_never_disposes() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(state.commands().0);

        state.ingest(&[frame(1, r(5.0, 0.0, 15.0, 10.0), true)]);
        assert!(
            !state
                .commands()
                .1
                .iter()
                .any(|c| matches!(c, ViewCommand::Dispose { .. }))
        );
    }

    // ---- D-8: backlog cap ---------------------------------------------------

    /// Drive `ingest` until the backlog cap trips (detected as the first poll
    /// where the backlog got *shorter*), never acknowledging. Returns the
    /// geometry offset of the last ingested frame. Panics rather than spinning
    /// forever if the cap somehow never trips.
    fn ingest_until_compaction(
        state: &mut PlatformViewState,
        mut frames_at: impl FnMut(f64) -> Vec<PlatformViewFrame>,
    ) -> f64 {
        let mut x = 0.0;
        loop {
            x += 1.0;
            assert!(x < 10_000.0, "backlog cap never tripped");
            let before = state.commands().1.len();
            state.ingest(&frames_at(x));
            if state.commands().1.len() < before {
                return x;
            }
        }
    }

    #[test]
    fn backlog_cap_compacts_into_a_live_slot_replay() {
        let mut state = PlatformViewState::new();
        let x = ingest_until_compaction(&mut state, |x| {
            vec![
                frame(1, r(x, 0.0, x + 10.0, 10.0), true),
                frame(2, r(x + 20.0, 0.0, x + 30.0, 10.0), true),
            ]
        });

        // Compacted into exactly the net effect: both slots re-created at their
        // latest geometry, nothing else.
        let (generation, cmds) = state.commands();
        assert!(cmds.len() <= MAX_PENDING_COMMANDS);
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(x, 0.0, x + 10.0, 10.0),
                    clip: None,
                    visible: true,
                },
                ViewCommand::Create {
                    slot_id: 2,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(x + 20.0, 0.0, x + 30.0, 10.0),
                    clip: None,
                    visible: true,
                },
            ]
        );

        // The whole compacted batch rides the current generation, so an ack of
        // an older one drops none of it and the current one drops all of it.
        state.acknowledge(generation - 1);
        assert_eq!(state.commands().1.len(), 4);
        state.acknowledge(generation);
        assert!(state.commands().1.is_empty());
    }

    #[test]
    fn backlog_cap_keeps_a_dropped_slots_dispose() {
        let mut state = PlatformViewState::new();
        // Slot 1 lives and dies inside the un-acked window; slot 2 stays live.
        state.ingest(&[
            frame(1, r(0.0, 0.0, 10.0, 10.0), true),
            frame(2, r(20.0, 0.0, 30.0, 10.0), true),
        ]);
        for _ in 0..DISPOSE_AFTER_MISSING_FRAMES {
            state.ingest(&[frame(2, r(20.0, 0.0, 30.0, 10.0), true)]);
        }
        assert!(
            state
                .commands()
                .1
                .contains(&ViewCommand::Dispose { slot_id: 1 })
        );

        ingest_until_compaction(&mut state, |x| {
            vec![frame(2, r(x + 20.0, 0.0, x + 30.0, 10.0), true)]
        });

        let (_, cmds) = state.commands();
        // The teardown of slot 1 survives compaction (else its native view
        // leaks); slot 2 is replayed.
        assert_eq!(cmds[0], ViewCommand::Dispose { slot_id: 1 });
        assert!(
            cmds.iter()
                .any(|c| matches!(c, ViewCommand::Create { slot_id: 2, .. }))
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, ViewCommand::Create { slot_id: 1, .. }))
        );
    }

    #[test]
    fn a_steadily_acking_native_side_never_trips_the_cap() {
        let mut state = PlatformViewState::new();
        for i in 0..1_000 {
            let x = i as f64;
            state.ingest(&[frame(1, r(x, 0.0, x + 10.0, 10.0), true)]);
            assert!(state.commands().1.len() <= MAX_PENDING_COMMANDS);
            state.acknowledge(state.commands().0);
        }
        assert!(state.commands().1.is_empty());
    }

    // ---- The release gate ---------------------------------------------------

    #[test]
    fn commands_up_to_releases_only_the_due_prefix() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        let gen1 = state.commands().0;
        state.ingest(&[frame(1, r(5.0, 0.0, 15.0, 10.0), true)]);
        let gen2 = state.commands().0;
        assert_eq!(state.commands().1.len(), 3);

        // Nothing due yet: an empty slice reported at the acked generation.
        assert_eq!(state.commands_up_to(0), (0, &[][..]));
        // The first batch's frame landed.
        let (reported, cmds) = state.commands_up_to(gen1);
        assert_eq!(reported, gen1);
        assert_eq!(cmds.len(), 2);
        // Both.
        let (reported, cmds) = state.commands_up_to(gen2);
        assert_eq!(reported, gen2);
        assert_eq!(cmds.len(), 3);
    }

    #[test]
    fn commands_up_to_reports_the_acked_generation_when_it_releases_nothing() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)]);
        state.acknowledge(state.commands().0);
        state.ingest(&[frame(1, r(9.0, 0.0, 19.0, 10.0), true)]);

        let (reported, cmds) = state.commands_up_to(1);
        assert!(cmds.is_empty());
        assert_eq!(reported, 1, "a held poll must not ack anything new");
    }

    #[test]
    fn gate_releases_a_batch_once_its_own_frame_is_presented() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 10);
        pairing.record(2, 11);

        // Frame 10 not on screen yet: nothing releasable (generation 1 - 1).
        assert_eq!(pairing.releasable_generation(9, 11), 0);
        // Frame 10 presented, 11 still in flight: only the first batch.
        assert_eq!(pairing.releasable_generation(10, 11), 1);
        // Both on screen: everything, including any later unpaired batch.
        assert_eq!(pairing.releasable_generation(11, 11), u64::MAX);
    }

    #[test]
    fn gate_releases_everything_before_the_first_held_batch() {
        let mut pairing = FramePairing::new();
        // A lifecycle batch (generation 1) was never paired with a frame; the
        // geometry batch behind it is held.
        pairing.record(2, 10);
        assert_eq!(pairing.releasable_generation(9, 10), 1);
    }

    #[test]
    fn gate_releases_everything_when_nothing_is_paired() {
        let pairing = FramePairing::new();
        assert!(pairing.is_empty());
        assert_eq!(pairing.releasable_generation(0, 0), u64::MAX);
    }

    #[test]
    fn escape_hatch_releases_a_batch_whose_frame_was_dropped() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 5); // frame 5's scene was overtaken and never presented
        pairing.record(2, 6);

        // Still within the in-flight window: held.
        let submitted = 5 + MAX_FRAMES_IN_FLIGHT - 1;
        assert_eq!(pairing.releasable_generation(4, submitted), 0);
        // One frame further and frame 5 is declared never-coming; frame 6 is
        // still inside the window, so the boundary stops there.
        let submitted = 5 + MAX_FRAMES_IN_FLIGHT;
        assert_eq!(pairing.releasable_generation(4, submitted), 1);
        // Both stale: everything releases.
        let submitted = 6 + MAX_FRAMES_IN_FLIGHT;
        assert_eq!(pairing.releasable_generation(4, submitted), u64::MAX);
    }

    #[test]
    fn gate_acknowledge_drops_applied_pairings() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 10);
        pairing.record(2, 11);
        pairing.acknowledge(1);
        // Generation 1's pairing is gone; generation 2 still gates.
        assert_eq!(pairing.releasable_generation(10, 11), 1);
        pairing.acknowledge(2);
        assert!(pairing.is_empty());
        assert_eq!(pairing.releasable_generation(0, 0), u64::MAX);
    }

    #[test]
    fn gate_clear_releases_a_suspend_or_replay_batch() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 10); // held: frame 10 never presents (app backgrounded)
        assert_eq!(pairing.releasable_generation(9, 10), 0);
        pairing.clear();
        assert_eq!(pairing.releasable_generation(9, 10), u64::MAX);
    }

    #[test]
    fn gate_tracking_is_bounded_by_an_unacking_native_side() {
        let mut pairing = FramePairing::new();
        for i in 1..=(MAX_TRACKED_BATCHES as u64 * 2) {
            pairing.record(i, i);
        }
        // Oldest entries were dropped, so the boundary is set by the oldest
        // SURVIVING batch rather than by a batch from the start of the run.
        let oldest_tracked = MAX_TRACKED_BATCHES as u64 + 1;
        assert_eq!(
            pairing.releasable_generation(0, oldest_tracked),
            oldest_tracked - 1
        );
    }

    #[test]
    fn deterministic_replay_produces_an_identical_command_stream() {
        let sequence: Vec<Vec<PlatformViewFrame>> = vec![
            vec![frame(1, r(0.0, 0.0, 10.0, 10.0), true)],
            vec![
                frame(1, r(0.0, 0.0, 10.0, 10.0), true),
                frame(2, r(20.0, 0.0, 30.0, 10.0), true),
            ],
            vec![frame(2, r(20.0, 0.0, 30.0, 10.0), true)], // slot 1 missing once
            vec![frame(2, r(20.0, 0.0, 30.0, 10.0), true)], // slot 1 missing twice: Hide
        ];

        let run = |sequence: &[Vec<PlatformViewFrame>]| -> Vec<ViewCommand> {
            let mut state = PlatformViewState::new();
            let mut all = Vec::new();
            for frames in sequence {
                state.ingest(frames);
                all.extend(state.commands().1.iter().cloned());
                // Compact after observing, mirroring a real shell's poll+ack.
                state.acknowledge(state.commands().0);
            }
            all
        };

        assert_eq!(run(&sequence), run(&sequence));
    }
}
