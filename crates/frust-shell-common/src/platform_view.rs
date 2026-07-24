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
//! # Skip-safety
//!
//! A gate-skipped frame (`docs/ARCHITECTURE.md`'s Frame gate) calls nothing —
//! a shell simply never calls [`PlatformViewState::ingest`] on a `Skip`
//! decision, so no rect can appear to "move" during a skip (paint doesn't run,
//! so `PaintCtx::visible_rect`/scroll state can't have changed either) —
//! nothing in this module special-cases a skip; the contract is entirely
//! "don't call ingest".

use std::collections::BTreeMap;

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
        self.push_batch(batch)
    }

    /// Snapshot for a shell's peek getter: the current generation plus the
    /// **entire** not-yet-acknowledged command backlog (not just the latest
    /// ingest's batch) — see the module docs' Generation/acknowledgement
    /// section.
    pub fn commands(&self) -> (u64, &[ViewCommand]) {
        (self.generation, &self.pending)
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
        true
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

    #[test]
    fn suspend_all_hides_every_visible_live_slot() {
        let mut state = PlatformViewState::new();
        state.ingest(&[
            frame(1, r(0.0, 0.0, 10.0, 10.0), true),
            frame(2, r(20.0, 0.0, 30.0, 10.0), false), // already hidden
        ]);
        state.acknowledge(state.commands().0);

        let changed = state.suspend_all();
        assert!(changed);
        let (_, cmds) = state.commands();
        // Only slot 1 (previously visible) gets a Hide; slot 2 was already
        // hidden, so it emits nothing.
        assert_eq!(
            cmds,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: false,
            }]
        );

        // A second call with nothing left visible is a no-op.
        state.acknowledge(state.commands().0);
        assert!(!state.suspend_all());
        assert_eq!(state.commands().1, &[]);
    }

    #[test]
    fn suspend_all_with_no_live_slots_is_a_no_op() {
        let mut state = PlatformViewState::new();
        assert!(!state.suspend_all());
        assert_eq!(state.commands(), (0, &[][..]));
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
