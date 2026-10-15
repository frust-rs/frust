//! Platform-agnostic native-sibling compositor logic: turns the raw,
//! per-paint-pass [`PlatformViewFrame`] collection (`frust-core`) into an
//! idempotent, generation-stamped command list — [`ViewCommand`] — both mobile
//! shells' FFI peek getters serve to their platform side
//! (`docs/SHELLS_ARCHITECTURE.md`'s platform-view embedding flow).
//!
//! Pure diffing logic with no FFI, no JSON, and no platform types — the same
//! "platform-agnostic brain, shell-owned wire format" split this crate draws
//! elsewhere (`frame_gate`'s skip decision, `resample`'s pointer
//! interpolation). JSON encoding of a [`ViewCommand`] batch stays hand-rolled
//! in each shell's own FFI glue (`docs/CODE_STANDARDS.md`'s "hand-roll JSON at
//! the mobile FFI boundary" rule); this module owns only the typed command
//! vocabulary, never `serde` or any wire format.
//!
//! # The `frust-core` → differ contract
//!
//! `frust-core::app::RenderRoot::platform_view_frames()` replaces its whole
//! `Vec<PlatformViewFrame>` every paint pass and stays deliberately dumb: a
//! slot absent from one pass's frames might be culled-but-still-alive,
//! momentarily not repainting, or genuinely torn down — core has no teardown
//! hook to tell those apart. [`PlatformViewState`] resolves that ambiguity by
//! watching how long a slot stays missing (`missing_streak`).
//!
//! # Command semantics
//!
//! - **New `slot_id`** ⇒ [`ViewCommand::Create`] then [`ViewCommand::Update`]
//!   in the same ingest batch, in that order — the native side never sees an
//!   `Update` for a view it hasn't been told to create yet.
//! - **Rect/clip/visible change past [`EPSILON_PX`]** ⇒ `Update`; a smaller
//!   change (or none at all) emits nothing, so a shell can call
//!   [`PlatformViewState::commands`] every frame for free when nothing moved.
//! - **`params_json` change** (detected via `params_generation`, bumped by the
//!   widget whenever it edits `params_json`) ⇒ [`ViewCommand::UpdateParams`],
//!   independent of the rect/clip/visible comparison above.
//! - **`view_type` change** on a live slot ⇒ [`ViewCommand::Dispose`] followed
//!   by a fresh `Create` + `Update`, **in the same ingest**. A different
//!   `view_type` is a different native factory, so the old view cannot be
//!   re-parameterized into the new one; emitting only a `Create` would be
//!   ignored by a host that already has a view for that slot id, and the
//!   [`DISPOSE_AFTER_MISSING_FRAMES`] streak would never fire at all (the slot
//!   is still present every pass). See the view-type-swap arm in
//!   [`PlatformViewState::ingest`].
//! - **Missing for [`HIDE_AFTER_MISSING_FRAMES`] consecutive ingests** (while
//!   the slot was last visible) ⇒ `Update { visible: false }` — a Hide. Only
//!   fires once per hide (the slot's tracked `last_visible` flips to `false`,
//!   so the same missing streak never re-emits it).
//! - **Missing for [`DISPOSE_AFTER_MISSING_FRAMES`] consecutive ingests** ⇒
//!   [`ViewCommand::Dispose`], and the slot is forgotten — a later
//!   reappearance of the same `slot_id` is indistinguishable from a brand-new
//!   one and gets a fresh `Create`. [`PlatformViewState::retire`] is the
//!   second, explicit path to the same outcome — the one a real widget
//!   teardown takes, immediately — and both are kept deliberately (Widget
//!   teardown detection, below).
//! - **Revive after Hide** (slot reappears in `ingest`'s frames before the
//!   dispose threshold): since the slot is still tracked, this is just an
//!   ordinary `Update` — `visible` flips back to `true` like any other
//!   changed field, no `Create`.
//! - **Revive after Dispose**: the slot was forgotten, so this is
//!   indistinguishable from new — fresh `Create` + `Update`.
//!
//! # Z-shields (`interactive` slots only)
//!
//! An interactive slot's `shields` list — the regions where frust content
//! painted OVER the slot keeps winning input — is assembled **here**, not by
//! the widget, from two sources:
//!
//! - the pass's auto-collected shield rects
//!   (`RenderRoot::input_shields()`, reported by `frust-widgets`' `shield(child)`
//!   wrapper), narrowed to those that overlap the slot's own `rect`; plus
//! - the slot's own manually declared rects (`PlatformViewView::shield_local`,
//!   the escape hatch), which arrive on the frame and are always kept.
//!
//! A **non-interactive** slot always ships an empty list: shields only mean
//! anything to a host that is forwarding touches to the native view in the
//! first place, so carrying them would be noise the host must ignore. The
//! resulting [`ViewCommand::Update`] shape is the same either way.
//!
//! Comparison is epsilon-based, like `rect`/`clip` (and order-sensitive: the
//! collection order is paint order, which is deterministic for an unchanged
//! tree), so a shield drifting sub-pixel with its chrome emits nothing.
//!
//! # Widget teardown detection
//!
//! Two paths converge on the same `Dispose`. A torn-down `platform_view`
//! widget reports its slot id to `frust-core`'s pending-retire list
//! (`RenderRoot::take_retired_platform_views`), which each shell drains after
//! its rebuild and feeds to [`PlatformViewState::retire`] — an immediate
//! `Dispose`, no streak. [`DISPOSE_AFTER_MISSING_FRAMES`] is the **backstop**
//! for what that hook cannot see (a widget dropped without `View::teardown`
//! running): a heuristic streak, since `ingest` alone cannot tell a dropped
//! widget from a culled or transiently-not-repainting one. Both paths give the
//! same command and the same "next `Create` is fresh" semantics, and a merely
//! culled slot reports no retire, so it correctly keeps living behind the
//! streak.
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
//! would otherwise grow it for the process lifetime — reachable, since a camera
//! preview is a genuinely long-lived slot. Past [`MAX_PENDING_COMMANDS`]
//! entries the backlog is **compacted into its own net effect**: one `Dispose`
//! per slot the dropped entries tore down, then a full `Create` + `Update`
//! replay of every live slot — exactly the surface-recreate replay
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
//! visibly ahead of the frust content it is supposed to be pinned to. A shell
//! that knows which frust frame each batch came from closes that gap by
//! releasing only the prefix whose frame is already presented:
//! [`FramePairing`] keeps the `(generation, frame_id)` bookkeeping and
//! [`commands_up_to`](PlatformViewState::commands_up_to) serves the prefix.
//! Holding geometry for a presentation that never comes is the gate's one
//! failure mode, so it releases anyway once the frame it waits on has fallen far
//! enough behind — counted in submissions while frames flow and in idle display
//! ticks ([`FramePairing::note_idle_tick`]) once they stop.
//!
//! # Z-order and overlays
//!
//! Every [`PlatformViewFrame`] carries its `command_index` — how many commands
//! the frame's scene had recorded when the slot published — so the frame's
//! slots have a painter order. [`ViewCommand::Update`]'s `z_index` is the
//! slot's rank in that order among the pass's slots (ties keep publication
//! order), so a host can stack its native views; a change of rank alone emits
//! an `Update`.
//!
//! Every command recorded at or past a slot's index paints **over** that
//! slot's native view, which a host compositing the native view above or
//! below the one frust surface cannot show. [`PlatformViewState::ingest_scene`]
//! therefore cuts the frame's command list at its *visible* slots' indices,
//! ordered `s0..sn`: overlay `i` covers the half-open range
//! `s_i.command_index..s_{i+1}.command_index` (the last one runs to the end of
//! the list), and exists only while that range holds at least one **drawing**
//! command ([`frust_scene::Command::is_drawing`]) — a range of clip/layer
//! brackets alone puts no pixels above the slot, and neither does the slot's
//! own Mode B hole-punch (the `ClearRect` recorded right after it publishes,
//! so at its own index: the punch still opens the range, but never makes the
//! overlay exist). Content painted before the
//! first visible slot stays on the main surface. Each live overlay is keyed by
//! the slot it sits above and announced with [`ViewCommand::Overlay`]
//! (its id, the slot, its rect — v1: the whole window — and its range), re-sent
//! whenever its range or rect changes. A hidden slot is not a cut: nothing
//! native shows there, so the content after it belongs to the overlay below it.
//!
//! Overlays share the slots' backlog, generation and acknowledgement, so the
//! release gate ([`FramePairing`]) pairs them with their frame exactly as it
//! does slot geometry. An overlay whose content disappears is re-sent once
//! with an **empty** range (draw nothing — its range must never point at a
//! later frame's unrelated commands) and is disposed
//! ([`ViewCommand::DisposeOverlay`]) after [`DISPOSE_AFTER_MISSING_FRAMES`]
//! consecutive ingests without content, or at once when the slot under it is
//! disposed. A frame with no platform views, or a shell calling plain
//! [`PlatformViewState::ingest`] (no command list), produces no overlays at all.
//!
//! A Mode B host (translucent main surface, native views composited *below*
//! frust) already shows frust content over a slot; it needs an overlay only for
//! the sandwich case — native view A above frust content X **and** frust
//! content Y above A. The differ computes overlays the same way regardless of
//! mode; the host decides which to realise. Input is unaffected: the z-shields
//! above already express "frust content above a slot" for hit-testing.
//!
//! # Skip-safety
//!
//! Nothing here special-cases a gate-skipped frame
//! (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate` module); the contract is
//! entirely "don't call [`ingest`](PlatformViewState::ingest) on a `Skip`".
//! Paint doesn't run on a skip, so no rect can appear to "move" either
//! (`PaintCtx::visible_rect`/scroll state can't have changed).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Range;

use frust_core::widget::PlatformViewFrame;
use frust_scene::Command;
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
/// the module docs' Widget teardown detection.
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
/// arm one dropped scene strands every later batch forever — a submission
/// counter is not a presented counter.
///
/// The same bound counts idle display ticks
/// ([`FramePairing::note_idle_tick`]), which is what keeps the hatch reachable
/// once the frame loop stops producing frames and the submission cursor freezes
/// with it.
///
/// Measured pipeline depth on physical test devices was 2–4 frames, so
/// 12 sits well above the working range while bounding worst-case staleness to
/// ~100 ms at 120 Hz.
pub const MAX_FRAMES_IN_FLIGHT: u64 = 12;

/// One native-sibling-compositor instruction — the differ's whole output
/// vocabulary. `Clone + PartialEq + Debug` so a golden
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
        /// Mode B input forwarding: whether a
        /// touch-DOWN inside this slot's rect hands the gesture to the
        /// native sibling. Fixed at create in v1 (no live flip).
        interactive: bool,
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
        /// The z-shield list: absolute-coordinate
        /// regions where frust content over the slot keeps winning input.
        /// Meaningful only for an interactive slot; empty otherwise.
        shields: Vec<Rect>,
        /// The slot's rank in painter order among the pass's slots (by
        /// `PlatformViewFrame::command_index`, ties in publication order):
        /// `0` is painted first, so a host stacks a higher `z_index` above a
        /// lower one. See the module docs' Z-order and overlays.
        z_index: u32,
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
    /// brand-new — see the module docs' Widget teardown detection.
    Dispose {
        /// Which slot to tear down.
        slot_id: u64,
    },
    /// Create or re-place the overlay carrying the frust content painted above
    /// `above_slot_id` — see the module docs' Z-order and overlays. A host
    /// that has no overlay `overlay_id` yet creates one; otherwise it adopts
    /// the new rect and range. An overlay stacks directly above its slot's
    /// native view and below any slot with a higher `z_index`.
    Overlay {
        /// Stable id for this overlay, allocated by the differ; never reused
        /// within one [`PlatformViewState`].
        overlay_id: u64,
        /// The slot this overlay sits directly above.
        above_slot_id: u64,
        /// Where the overlay covers, logical px, absolute window coordinates.
        /// v1: the whole window (the viewport the shell passed to
        /// [`PlatformViewState::ingest_scene`]).
        rect: Rect,
        /// The half-open range of the frame's command list (the slice passed
        /// to [`PlatformViewState::ingest_scene`]) the overlay renders, in
        /// painter order. Empty when the overlay's content has disappeared —
        /// it then draws nothing until it is re-sent or disposed.
        command_range: Range<usize>,
    },
    /// Tear an overlay down. A `Dispose`-style no-op for an id the host does
    /// not have.
    DisposeOverlay {
        /// Which overlay to tear down.
        overlay_id: u64,
    },
}

/// Per-slot last-emitted state the differ compares each `ingest` call
/// against, to decide whether anything actually changed.
#[derive(Clone, Debug)]
struct SlotEntry {
    view_type: String,
    params_json: String,
    params_generation: u64,
    interactive: bool,
    last_rect: Rect,
    last_clip: Option<Rect>,
    last_visible: bool,
    last_shields: Vec<Rect>,
    last_z_index: u32,
    /// Consecutive `ingest` calls this slot has been absent from `frames`.
    /// Reset to `0` the instant it reappears.
    missing_streak: u32,
}

/// Per-overlay last-emitted state, keyed in [`PlatformViewState`] by the slot
/// the overlay sits above.
#[derive(Clone, Debug)]
struct OverlayEntry {
    overlay_id: u64,
    last_rect: Rect,
    last_range: Range<usize>,
    /// Consecutive ingests this overlay has had no drawing content.
    missing_streak: u32,
}

/// The differ: per-slot last-seen state plus the accumulated, not-yet-acknowledged
/// [`ViewCommand`] backlog. See the module docs for the full semantics.
///
/// `live` is a [`BTreeMap`] (keyed by `slot_id`), not a `HashMap` — iteration
/// order must be deterministic (ascending `slot_id`) for the "same ingest
/// sequence ⇒ identical command stream" golden-test guarantee; a frame's own
/// `Create`+`Update` ordering is separately
/// guaranteed by iterating `frames` itself in the caller's given order.
#[derive(Debug, Default)]
pub struct PlatformViewState {
    live: BTreeMap<u64, SlotEntry>,
    /// Live overlays keyed by the slot they sit above (a `BTreeMap` for the
    /// same determinism `live` needs).
    overlays: BTreeMap<u64, OverlayEntry>,
    /// The last overlay id handed out; ids start at `1`.
    last_overlay_id: u64,
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

    /// Feed one paint pass's frames (`RenderRoot::platform_view_frames()`) and
    /// that same pass's z-shield rects (`RenderRoot::input_shields()`).
    /// Returns `true` if this call produced at least one command (i.e. the
    /// generation advanced) — a caller that only cares "did anything change"
    /// can skip calling [`commands`](Self::commands) entirely when this is
    /// `false`.
    ///
    /// `input_shields` is a flat, slot-agnostic list (core never associates a
    /// shield with a slot); this method owns the intersection rule — see the
    /// module docs' Z-shields section. Pass `&[]` when a caller has no shield
    /// channel: every slot then ships only its own manually declared rects.
    ///
    /// Do not call this on a gate-skipped frame — see the module docs'
    /// Skip-safety section.
    ///
    /// Knows nothing of the frame's commands, so it computes no overlays: it
    /// is [`ingest_scene`](Self::ingest_scene) with an empty command list, and
    /// any overlay a previous `ingest_scene` created lapses as content-less.
    pub fn ingest(&mut self, frames: &[PlatformViewFrame], input_shields: &[Rect]) -> bool {
        self.ingest_scene(frames, input_shields, &[], Rect::ZERO)
    }

    /// [`ingest`](Self::ingest) plus the overlay pass (module docs' Z-order and
    /// overlays): `commands` is the painted scene's command list
    /// (`Scene::commands()`, the same frame `frames` came from) and `viewport`
    /// the window's logical bounds, which every v1 overlay covers.
    ///
    /// The command slice is borrowed, not copied: the overlay pass scans only
    /// the ranges past the first visible slot, stopping at each range's first
    /// drawing command, so no per-frame classification buffer is built. A
    /// frame without platform views scans nothing.
    pub fn ingest_scene(
        &mut self,
        frames: &[PlatformViewFrame],
        input_shields: &[Rect],
        commands: &[Command],
        viewport: Rect,
    ) -> bool {
        let mut batch = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let order = paint_order(frames);
        let mut z_of = vec![0u32; frames.len()];
        for (rank, &i) in order.iter().enumerate() {
            z_of[i] = u32::try_from(rank).unwrap_or(u32::MAX);
        }

        for (i, frame) in frames.iter().enumerate() {
            let z_index = z_of[i];
            seen.insert(frame.slot_id);
            // The shields this slot actually ships this pass (module docs'
            // Z-shields): its own manual rects plus the auto-collected ones
            // overlapping it, or nothing at all when it isn't interactive.
            let shields = resolve_shields(frame, input_shields);
            // The slot's `view_type` changed under a live id. A different
            // `view_type` resolves to a different native factory, so the old
            // view must be torn down and a new one built — in THIS batch. Drop
            // the tracked entry first, so the `None` arm below emits the fresh
            // `Create` + `Update` after the `Dispose`, exactly as it would for
            // an id it had never seen.
            if let Some(entry) = self.live.get(&frame.slot_id)
                && entry.view_type != frame.view_type
            {
                // The overlay above the old view goes with it; the overlay
                // pass below re-creates one (fresh id) if content still sits
                // above the new view.
                dispose_overlay_above(&mut self.overlays, frame.slot_id, &mut batch);
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
                        interactive: frame.interactive,
                    });
                    batch.push(ViewCommand::Update {
                        slot_id: frame.slot_id,
                        rect: frame.rect,
                        clip: frame.clip,
                        visible: frame.visible,
                        shields: shields.clone(),
                        z_index,
                    });
                    self.live.insert(
                        frame.slot_id,
                        SlotEntry {
                            view_type: frame.view_type.clone(),
                            params_json: frame.params_json.clone(),
                            params_generation: frame.params_generation,
                            interactive: frame.interactive,
                            last_rect: frame.rect,
                            last_clip: frame.clip,
                            last_visible: frame.visible,
                            last_shields: shields,
                            last_z_index: z_index,
                            missing_streak: 0,
                        },
                    );
                }
                Some(entry) => {
                    entry.missing_streak = 0;
                    if entry.last_visible != frame.visible
                        || rect_changed(entry.last_rect, frame.rect)
                        || clip_changed(entry.last_clip, frame.clip)
                        || shields_changed(&entry.last_shields, &shields)
                        || entry.last_z_index != z_index
                    {
                        batch.push(ViewCommand::Update {
                            slot_id: frame.slot_id,
                            rect: frame.rect,
                            clip: frame.clip,
                            visible: frame.visible,
                            shields: shields.clone(),
                            z_index,
                        });
                        entry.last_rect = frame.rect;
                        entry.last_clip = frame.clip;
                        entry.last_visible = frame.visible;
                        entry.last_shields = shields;
                        entry.last_z_index = z_index;
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
                    shields: entry.last_shields.clone(),
                    z_index: entry.last_z_index,
                });
                entry.last_visible = false;
            }
            if entry.missing_streak >= DISPOSE_AFTER_MISSING_FRAMES {
                dispose_overlay_above(&mut self.overlays, slot_id, &mut batch);
                batch.push(ViewCommand::Dispose { slot_id });
                disposed.push(slot_id);
            }
        }
        for slot_id in disposed {
            self.live.remove(&slot_id);
        }

        self.ingest_overlays(frames, &order, commands, viewport, &mut batch);

        self.push_batch(batch)
    }

    /// The overlay pass (module docs' Z-order and overlays): cut `commands` at
    /// the visible slots' indices in painter order, (re)announce every overlay
    /// whose range holds a drawing command, and age the rest toward disposal.
    fn ingest_overlays(
        &mut self,
        frames: &[PlatformViewFrame],
        order: &[usize],
        commands: &[Command],
        viewport: Rect,
        batch: &mut Vec<ViewCommand>,
    ) {
        let total = commands.len();
        let mut cuts = order
            .iter()
            .map(|&i| &frames[i])
            .filter(|frame| frame.visible)
            .peekable();
        let mut with_content = BTreeSet::new();
        while let Some(slot) = cuts.next() {
            let start = slot.command_index.min(total);
            let end = cuts
                .peek()
                .map_or(total, |next| next.command_index.min(total))
                .max(start);
            // A Mode B slot's own hole-punch is recorded right after it
            // publishes, so it sits AT the slot's index (a `ClearRect` has no
            // other producer). It erases what is beneath the slot rather than
            // painting over it, so it never makes an overlay exist on its own.
            let content_from = match commands.get(start) {
                Some(Command::ClearRect { .. }) => (start + 1).min(end),
                _ => start,
            };
            if !commands[content_from..end].iter().any(Command::is_drawing) {
                continue;
            }
            let range = start..end;
            with_content.insert(slot.slot_id);
            match self.overlays.get_mut(&slot.slot_id) {
                Some(entry) => {
                    entry.missing_streak = 0;
                    if entry.last_range != range || rect_changed(entry.last_rect, viewport) {
                        batch.push(ViewCommand::Overlay {
                            overlay_id: entry.overlay_id,
                            above_slot_id: slot.slot_id,
                            rect: viewport,
                            command_range: range.clone(),
                        });
                        entry.last_range = range;
                        entry.last_rect = viewport;
                    }
                }
                None => {
                    self.last_overlay_id += 1;
                    let overlay_id = self.last_overlay_id;
                    batch.push(ViewCommand::Overlay {
                        overlay_id,
                        above_slot_id: slot.slot_id,
                        rect: viewport,
                        command_range: range.clone(),
                    });
                    self.overlays.insert(
                        slot.slot_id,
                        OverlayEntry {
                            overlay_id,
                            last_rect: viewport,
                            last_range: range,
                            missing_streak: 0,
                        },
                    );
                }
            }
        }

        let mut disposed = Vec::new();
        for (&slot_id, entry) in self.overlays.iter_mut() {
            if with_content.contains(&slot_id) {
                continue;
            }
            entry.missing_streak += 1;
            if entry.missing_streak >= DISPOSE_AFTER_MISSING_FRAMES {
                batch.push(ViewCommand::DisposeOverlay {
                    overlay_id: entry.overlay_id,
                });
                disposed.push(slot_id);
            } else if !entry.last_range.is_empty() {
                // Content gone: empty the range at once so the overlay never
                // renders a later frame's unrelated commands at stale indices.
                let empty = entry.last_range.start..entry.last_range.start;
                batch.push(ViewCommand::Overlay {
                    overlay_id: entry.overlay_id,
                    above_slot_id: slot_id,
                    rect: entry.last_rect,
                    command_range: empty.clone(),
                });
                entry.last_range = empty;
            }
        }
        for slot_id in disposed {
            self.overlays.remove(&slot_id);
        }
    }

    /// Backgrounding path: synthesize
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
                    shields: entry.last_shields.clone(),
                    z_index: entry.last_z_index,
                });
                entry.last_visible = false;
            }
        }
        self.push_batch(batch)
    }

    /// Explicit retire: dispose `slot_id` right now regardless of its missing
    /// streak, for a shell with a real teardown signal (see the module docs'
    /// Widget teardown detection). A no-op (returns `false`) if
    /// `slot_id` isn't currently live (already disposed, or never created).
    /// The overlay above the slot, if any, is disposed with it, first.
    pub fn retire(&mut self, slot_id: u64) -> bool {
        if self.live.remove(&slot_id).is_some() {
            let mut batch = Vec::new();
            dispose_overlay_above(&mut self.overlays, slot_id, &mut batch);
            batch.push(ViewCommand::Dispose { slot_id });
            self.push_batch(batch)
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
    /// `slot_id`, each preserving its last-known rect/clip/visibility), then an
    /// `Overlay` for every live overlay (ascending above-slot id) — the
    /// shared body of [`reset_for_surface_recreate`](Self::reset_for_surface_recreate)
    /// and the backlog-cap compaction (module docs' Backlog cap), which are the
    /// same "rebuild everything from this batch alone" statement.
    fn replay_live_into(&self, batch: &mut Vec<ViewCommand>) {
        for (&slot_id, entry) in self.live.iter() {
            batch.push(ViewCommand::Create {
                slot_id,
                view_type: entry.view_type.clone(),
                params_json: entry.params_json.clone(),
                interactive: entry.interactive,
            });
            batch.push(ViewCommand::Update {
                slot_id,
                rect: entry.last_rect,
                clip: entry.last_clip,
                visible: entry.last_visible,
                shields: entry.last_shields.clone(),
                z_index: entry.last_z_index,
            });
        }
        for (&above_slot_id, entry) in self.overlays.iter() {
            batch.push(ViewCommand::Overlay {
                overlay_id: entry.overlay_id,
                above_slot_id,
                rect: entry.last_rect,
                command_range: entry.last_range.clone(),
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
    /// `DisposeOverlay` per overlay the backlog tore down (ascending id), one
    /// `Dispose` per slot the backlog tore down (ascending `slot_id`), then a
    /// full replay of every live slot and overlay.
    ///
    /// The `Dispose`s must survive: a slot created *before* the un-acked window
    /// and disposed inside it is a native view the host already built and would
    /// otherwise never be told to tear down — a leak. A slot that was disposed
    /// **and** is live again (a `slot_id` reuse, or a `view_type` swap)
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
        let disposed_overlays: BTreeSet<u64> = self
            .pending
            .iter()
            .filter_map(|cmd| match cmd {
                ViewCommand::DisposeOverlay { overlay_id } => Some(*overlay_id),
                _ => None,
            })
            .collect();
        let mut compacted = Vec::with_capacity(
            disposed_overlays.len() + disposed.len() + self.live.len() * 2 + self.overlays.len(),
        );
        for overlay_id in disposed_overlays {
            compacted.push(ViewCommand::DisposeOverlay { overlay_id });
        }
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
/// the id of the frame that actually presented does not. The
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
///
/// Going *idle* is the third such moment, and the only one with no lifecycle
/// callback to hang a `clear` on: the loop simply stops producing frames while
/// the display keeps ticking. A shell reports those ticks
/// ([`note_idle_tick`](Self::note_idle_tick)) so the staleness hatch stays
/// reachable there — without them a batch whose frame never presented is held
/// for the process lifetime (see that method for the full failure mode).
#[derive(Debug, Default)]
pub struct FramePairing {
    /// `(generation, frame_id)` per produced batch, oldest first. Both
    /// components are monotonically non-decreasing along the queue, which is
    /// what lets [`releasable_generation`](Self::releasable_generation) stop at
    /// the first held entry.
    due: VecDeque<(u64, u64)>,
    /// Display ticks that produced no frust frame since the last
    /// [`record`](Self::record) — the idle half of the staleness cursor (see
    /// [`note_idle_tick`](Self::note_idle_tick)). Reset by every produced batch
    /// and by [`clear`](Self::clear), so it only ever measures the *current*
    /// idle stretch: an entry recorded before an earlier stretch is aged by
    /// fewer ticks than really elapsed, which errs toward holding, never toward
    /// releasing early.
    idle_ticks: u64,
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
        // A produced frame ends the idle stretch: this batch's own frame is
        // genuinely in flight, so from here the submission cursor is the honest
        // clock to age every entry by again.
        self.idle_ticks = 0;
    }

    /// Report one display tick on which the frame loop produced **no** frust
    /// frame — the tick a shell's frame gate skipped. Ages every held batch
    /// exactly as a submission does (see
    /// [`releasable_generation`](Self::releasable_generation)).
    ///
    /// **Why the release gate needs an idle clock at all.** A batch is released
    /// on one of two events: its own frame is confirmed *presented*, or the
    /// submission cursor climbs [`MAX_FRAMES_IN_FLIGHT`] past it. A present is
    /// recorded only for a `Rendered` render outcome, so any other one — an
    /// encode or acquire skipped against a surface that is not ready, a
    /// swapchain reconfigure, a lost surface, an encode/acquire error — leaves
    /// the batch's frame permanently unconfirmed. That is survivable while
    /// frames keep flowing, because the submission cursor walks past it within
    /// twelve frames. Once the app settles, though, the submission cursor stops
    /// too, and *neither* arm can ever fire again: the settled geometry is held
    /// for the process lifetime and the native sibling stays parked at whatever
    /// mid-animation rect it last applied — device-observed as a camera preview
    /// stuck black behind correct-but-never-delivered geometry, healed only by a
    /// surface recreate (which `clear`s the pairing). The display clock is the
    /// one cursor still moving at idle, and an idle tick carries exactly the
    /// evidence the submission cursor does: that frame is not coming.
    ///
    /// **Why an idle *bound* and not an immediate release.** Releasing the whole
    /// backlog on the last painted frame is not expressible: a touch-driven drag
    /// paints with `needs_frame == false` every frame, so "this paint asked for
    /// no continuation frame" cannot tell a settle frame from a mid-drag one,
    /// and keying the release on it would turn the gate off for exactly the
    /// scrolling case it was built to smooth. Aging by idle ticks costs nothing
    /// on any path where frames still flow — a present that does arrive still
    /// releases the batch first, unchanged — and bounds the broken path to
    /// [`MAX_FRAMES_IN_FLIGHT`] display ticks (~100 ms at 120 Hz).
    pub fn note_idle_tick(&mut self) {
        self.idle_ticks = self.idle_ticks.saturating_add(1);
    }

    /// The highest generation releasable right now, given the id of the last
    /// **presented** frame and of the last **submitted** one.
    ///
    /// A batch is releasable once its own frame is on screen, or once that
    /// frame has fallen [`MAX_FRAMES_IN_FLIGHT`] behind the staleness cursor
    /// (it was dropped by the latest-wins channel, or never presented at all,
    /// and will never reach the screen). The first batch that is neither caps
    /// the boundary at its own generation minus one, so everything published
    /// before it — including a lifecycle batch that was never paired with a
    /// frame at all — still goes out; an empty queue releases everything.
    ///
    /// The staleness cursor is the submission cursor plus the current idle
    /// stretch ([`note_idle_tick`](Self::note_idle_tick)): the two are the same
    /// "frames have moved on past this one" evidence, and with no idle ticks
    /// reported this is bit-for-bit the submission-only rule.
    pub fn releasable_generation(&self, presented_frame_id: u64, submitted_frame_id: u64) -> u64 {
        let stale_cursor = submitted_frame_id.saturating_add(self.idle_ticks);
        for &(generation, due_frame) in &self.due {
            let on_screen = due_frame <= presented_frame_id;
            let stranded = stale_cursor.saturating_sub(due_frame) >= MAX_FRAMES_IN_FLIGHT;
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
    /// type's Lifecycle note. Also drops the idle stretch, so the ticks counted
    /// against frames belonging to a surface (or a foreground session) that is
    /// gone cannot age the first batch recorded after it.
    pub fn clear(&mut self) {
        self.due.clear();
        self.idle_ticks = 0;
    }

    /// Whether any batch is still waiting to be paired off.
    pub fn is_empty(&self) -> bool {
        self.due.is_empty()
    }
}

/// Indices into `frames` in painter order: ascending `command_index`, ties in
/// publication order (a stable sort). Allocates nothing for an empty pass.
fn paint_order(frames: &[PlatformViewFrame]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..frames.len()).collect();
    order.sort_by_key(|&i| frames[i].command_index);
    order
}

/// Drop the overlay sitting above `slot_id`, if any, queueing its
/// `DisposeOverlay` — the overlay half of a slot disposal (module docs'
/// Z-order and overlays).
fn dispose_overlay_above(
    overlays: &mut BTreeMap<u64, OverlayEntry>,
    slot_id: u64,
    batch: &mut Vec<ViewCommand>,
) {
    if let Some(entry) = overlays.remove(&slot_id) {
        batch.push(ViewCommand::DisposeOverlay {
            overlay_id: entry.overlay_id,
        });
    }
}

/// The shield list a slot ships this pass (module docs' Z-shields): nothing at
/// all for a non-interactive slot, else its own manually declared rects
/// (`PlatformViewView::shield_local`, already absolute) plus every
/// auto-collected rect overlapping the slot's `rect`.
///
/// Overlap (not a positive-area intersection) is the same edge-inclusive test
/// the widget's own visibility check uses; a duplicate — the same region
/// declared manually AND painted by a `shield` wrapper — is dropped by the
/// epsilon comparison, so a host never sees the same rect twice.
fn resolve_shields(frame: &PlatformViewFrame, input_shields: &[Rect]) -> Vec<Rect> {
    if !frame.interactive {
        return Vec::new();
    }
    let mut resolved = frame.shields.clone();
    for shield in input_shields {
        if shield.overlaps(frame.rect) && !resolved.iter().any(|kept| !rect_changed(*kept, *shield))
        {
            resolved.push(*shield);
        }
    }
    resolved
}

/// Whether two shield lists differ past [`EPSILON_PX`]. Order-sensitive: the
/// list is built in paint order, which is stable for an unchanged tree, so a
/// reorder legitimately means the shields moved.
fn shields_changed(a: &[Rect], b: &[Rect]) -> bool {
    a.len() != b.len() || a.iter().zip(b).any(|(a, b)| rect_changed(*a, *b))
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
            interactive: false,
            shields: Vec::new(),
            command_index: 0,
        }
    }

    fn r(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    /// A frame for an `interactive` slot (the only kind that ships shields).
    fn interactive_frame(slot_id: u64, rect: Rect) -> PlatformViewFrame {
        PlatformViewFrame {
            interactive: true,
            ..frame(slot_id, rect, true)
        }
    }

    /// The `shields` list of the last [`ViewCommand::Update`] in the backlog.
    fn last_update_shields(state: &PlatformViewState) -> Vec<Rect> {
        state
            .commands()
            .1
            .iter()
            .rev()
            .find_map(|cmd| match cmd {
                ViewCommand::Update { shields, .. } => Some(shields.clone()),
                _ => None,
            })
            .expect("an Update command in the backlog")
    }

    #[test]
    fn new_slot_emits_create_then_update_in_order() {
        let mut state = PlatformViewState::new();
        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 100.0, 50.0), true)], &[]);
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
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 100.0, 50.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 0,
                },
            ]
        );
    }

    #[test]
    fn sub_epsilon_rect_change_emits_nothing() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 100.0, 50.0), true)], &[]);
        state.acknowledge(1);

        // 0.2px drift on every edge — below the 0.5px epsilon.
        let changed = state.ingest(&[frame(1, r(0.2, 0.2, 100.2, 50.2), true)], &[]);
        assert!(!changed);
        let (generation, cmds) = state.commands();
        assert_eq!(generation, 1); // unchanged — no new batch
        assert!(cmds.is_empty());
    }

    #[test]
    fn past_epsilon_rect_change_emits_update() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 100.0, 50.0), true)], &[]);
        state.acknowledge(1);

        let changed = state.ingest(&[frame(1, r(1.0, 0.0, 100.0, 50.0), true)], &[]);
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
                shields: Vec::new(),
                z_index: 0,
            }]
        );
    }

    #[test]
    fn missing_two_consecutive_ingests_hides_a_visible_slot() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(1);

        // Missing once: no hide yet.
        let changed = state.ingest(&[], &[]);
        assert!(!changed);
        assert_eq!(state.commands().1, &[]);

        // Missing twice consecutively: Hide.
        let changed = state.ingest(&[], &[]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: false,
                shields: Vec::new(),
                z_index: 0,
            }]
        );

        // A further missing ingest doesn't re-emit the same Hide.
        state.acknowledge(state.commands().0);
        let changed = state.ingest(&[], &[]);
        assert!(!changed);
    }

    #[test]
    fn missing_past_dispose_threshold_disposes_the_slot() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(1);

        for _ in 0..(DISPOSE_AFTER_MISSING_FRAMES - 1) {
            state.ingest(&[], &[]);
        }
        // Not yet disposed at streak == DISPOSE_AFTER_MISSING_FRAMES - 1.
        let (_, cmds) = state.commands();
        assert!(!cmds.contains(&ViewCommand::Dispose { slot_id: 1 }));

        let changed = state.ingest(&[], &[]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert!(cmds.contains(&ViewCommand::Dispose { slot_id: 1 }));
    }

    #[test]
    fn revive_after_hide_is_a_plain_update_not_a_create() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.ingest(&[], &[]); // streak 1
        state.ingest(&[], &[]); // streak 2: Hide
        state.acknowledge(state.commands().0);

        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: true,
                shields: Vec::new(),
                z_index: 0,
            }]
        );
        assert!(!cmds.iter().any(|c| matches!(c, ViewCommand::Create { .. })));
    }

    #[test]
    fn revive_after_dispose_creates_fresh() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        for _ in 0..DISPOSE_AFTER_MISSING_FRAMES {
            state.ingest(&[], &[]);
        }
        state.acknowledge(state.commands().0);

        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 0,
                },
            ]
        );
    }

    #[test]
    fn acknowledge_compacts_the_backlog() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        let (gen1, _) = state.commands();
        state.ingest(&[frame(2, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
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
        state.ingest(
            &[
                frame(1, r(0.0, 0.0, 10.0, 10.0), true),
                frame(2, r(20.0, 0.0, 30.0, 10.0), false),
            ],
            &[],
        );
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
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 0,
                },
                ViewCommand::Create {
                    slot_id: 2,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(20.0, 0.0, 30.0, 10.0),
                    clip: None,
                    visible: false,
                    shields: Vec::new(),
                    z_index: 1,
                },
            ]
        );
    }

    #[test]
    fn two_slot_interleaving_does_not_cross_contaminate() {
        let mut state = PlatformViewState::new();
        // Slot 1 created; slot 2 not yet present.
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);

        // Slot 2 appears; slot 1 unchanged (no rect/clip/visible drift) — only
        // slot 2's Create+Update should be emitted.
        let changed = state.ingest(
            &[
                frame(1, r(0.0, 0.0, 10.0, 10.0), true),
                frame(2, r(50.0, 50.0, 60.0, 60.0), true),
            ],
            &[],
        );
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[
                ViewCommand::Create {
                    slot_id: 2,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(50.0, 50.0, 60.0, 60.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 1,
                },
            ]
        );
        state.acknowledge(state.commands().0);

        // Now slot 1 moves and slot 2 disappears (one missing frame — not yet
        // hidden): only slot 1's Update should appear.
        let changed = state.ingest(&[frame(1, r(5.0, 0.0, 15.0, 10.0), true)], &[]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(5.0, 0.0, 15.0, 10.0),
                clip: None,
                visible: true,
                shields: Vec::new(),
                z_index: 0,
            }]
        );
    }

    #[test]
    fn params_generation_change_emits_update_params() {
        let mut state = PlatformViewState::new();
        let mut f = frame(1, r(0.0, 0.0, 10.0, 10.0), true);
        f.params_json = "{\"a\":1}".to_string();
        state.ingest(&[f], &[]);
        state.acknowledge(state.commands().0);

        let mut f2 = frame(1, r(0.0, 0.0, 10.0, 10.0), true);
        f2.params_json = "{\"a\":2}".to_string();
        f2.params_generation = 1;
        let changed = state.ingest(&[f2], &[]);
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
        state.ingest(
            &[
                frame(1, r(0.0, 0.0, 10.0, 10.0), true),
                frame(2, r(20.0, 0.0, 30.0, 10.0), true),
            ],
            &[],
        );
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
                    shields: Vec::new(),
                    z_index: 0,
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(20.0, 0.0, 30.0, 10.0),
                    clip: None,
                    visible: false,
                    shields: Vec::new(),
                    z_index: 1,
                },
            ]
        );
    }

    #[test]
    fn suspend_all_skips_already_hidden_slots() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), false)], &[]);
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
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);
        state.suspend_all();
        state.acknowledge(state.commands().0);

        // The slot reappears in the next real ingest (e.g. the first frame
        // after resume) — an ordinary Update, no re-Create.
        let changed = state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        assert!(changed);
        let (_, cmds) = state.commands();
        assert_eq!(
            cmds,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: r(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: true,
                shields: Vec::new(),
                z_index: 0,
            }]
        );
        assert!(!cmds.iter().any(|c| matches!(c, ViewCommand::Create { .. })));
    }

    #[test]
    fn retire_disposes_immediately_regardless_of_missing_streak() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);

        assert!(state.retire(1));
        assert_eq!(state.commands().1, &[ViewCommand::Dispose { slot_id: 1 }]);

        // Retiring an already-gone (or never-created) slot is a no-op.
        assert!(!state.retire(1));
        assert!(!state.retire(999));
    }

    // ---- view_type swap -----------------------------------------------------

    #[test]
    fn view_type_swap_disposes_and_recreates_in_the_same_ingest() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);

        let mut swapped = frame(1, r(0.0, 0.0, 10.0, 10.0), true);
        swapped.view_type = "dev.frust.Other".to_string();
        let changed = state.ingest(&[swapped], &[]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[
                ViewCommand::Dispose { slot_id: 1 },
                ViewCommand::Create {
                    slot_id: 1,
                    view_type: "dev.frust.Other".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 0,
                },
            ]
        );
    }

    #[test]
    fn view_type_swap_does_not_disturb_other_slots() {
        let mut state = PlatformViewState::new();
        state.ingest(
            &[
                frame(1, r(0.0, 0.0, 10.0, 10.0), true),
                frame(2, r(20.0, 0.0, 30.0, 10.0), true),
            ],
            &[],
        );
        state.acknowledge(state.commands().0);

        let mut swapped = frame(2, r(20.0, 0.0, 30.0, 10.0), true);
        swapped.view_type = "dev.frust.Other".to_string();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true), swapped], &[]);
        let (_, cmds) = state.commands();
        assert!(cmds.iter().all(|c| match c {
            ViewCommand::Create { slot_id, .. }
            | ViewCommand::Update { slot_id, .. }
            | ViewCommand::UpdateParams { slot_id, .. }
            | ViewCommand::Dispose { slot_id } => *slot_id == 2,
            ViewCommand::Overlay { .. } | ViewCommand::DisposeOverlay { .. } => false,
        }));
    }

    #[test]
    fn unchanged_view_type_never_disposes() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);

        state.ingest(&[frame(1, r(5.0, 0.0, 15.0, 10.0), true)], &[]);
        assert!(
            !state
                .commands()
                .1
                .iter()
                .any(|c| matches!(c, ViewCommand::Dispose { .. }))
        );
    }

    // ---- backlog cap --------------------------------------------------------

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
            state.ingest(&frames_at(x), &[]);
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
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 1,
                    rect: r(x, 0.0, x + 10.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 0,
                },
                ViewCommand::Create {
                    slot_id: 2,
                    view_type: "dev.frust.Test".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                ViewCommand::Update {
                    slot_id: 2,
                    rect: r(x + 20.0, 0.0, x + 30.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                    z_index: 1,
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
        state.ingest(
            &[
                frame(1, r(0.0, 0.0, 10.0, 10.0), true),
                frame(2, r(20.0, 0.0, 30.0, 10.0), true),
            ],
            &[],
        );
        for _ in 0..DISPOSE_AFTER_MISSING_FRAMES {
            state.ingest(&[frame(2, r(20.0, 0.0, 30.0, 10.0), true)], &[]);
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
            state.ingest(&[frame(1, r(x, 0.0, x + 10.0, 10.0), true)], &[]);
            assert!(state.commands().1.len() <= MAX_PENDING_COMMANDS);
            state.acknowledge(state.commands().0);
        }
        assert!(state.commands().1.is_empty());
    }

    // ---- The release gate ---------------------------------------------------

    #[test]
    fn commands_up_to_releases_only_the_due_prefix() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        let gen1 = state.commands().0;
        state.ingest(&[frame(1, r(5.0, 0.0, 15.0, 10.0), true)], &[]);
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
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);
        state.ingest(&[frame(1, r(9.0, 0.0, 19.0, 10.0), true)], &[]);

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
    fn idle_ticks_release_a_batch_whose_frame_never_presents() {
        let mut pairing = FramePairing::new();
        // The settle frame: batch 1 rides frame 10, which is submitted and then
        // never presented (any non-`Rendered` render outcome records nothing).
        pairing.record(1, 10);
        assert_eq!(
            pairing.releasable_generation(9, 10),
            0,
            "held while that frame could still land"
        );

        // The app is now idle — no further submissions, so the display clock is
        // the only cursor left moving.
        for _ in 0..(MAX_FRAMES_IN_FLIGHT - 1) {
            pairing.note_idle_tick();
            assert_eq!(
                pairing.releasable_generation(9, 10),
                0,
                "still inside the staleness bound"
            );
        }
        pairing.note_idle_tick();
        assert_eq!(
            pairing.releasable_generation(9, 10),
            u64::MAX,
            "the idle stretch strands a frame that will never present"
        );
    }

    #[test]
    fn an_idle_released_batch_is_served_exactly_once() {
        let mut state = PlatformViewState::new();
        let mut pairing = FramePairing::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        let generation = state.commands().0;
        pairing.record(generation, 10);

        // Frame 10 never presented and nothing else was submitted: the poll
        // serves nothing at all.
        let releasable = pairing.releasable_generation(9, 10);
        assert!(state.commands_up_to(releasable).1.is_empty());

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            pairing.note_idle_tick();
        }
        let releasable = pairing.releasable_generation(9, 10);
        let (reported, cmds) = state.commands_up_to(releasable);
        assert_eq!(cmds.len(), 2, "the held Create+Update finally go out");
        assert_eq!(reported, generation);

        // The native side applies and acks: both halves compact, so no further
        // poll — however many more idle ticks land — re-serves the batch.
        state.acknowledge(reported);
        pairing.acknowledge(reported);
        pairing.note_idle_tick();
        let releasable = pairing.releasable_generation(9, 10);
        assert!(
            state.commands_up_to(releasable).1.is_empty(),
            "applied once, never re-applied"
        );
        assert!(pairing.is_empty());
    }

    #[test]
    fn a_presented_frame_releases_before_the_idle_bound_is_reached() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 10);
        // The render tail runs a tick or two behind the UI thread, so a settle
        // frame's present routinely lands after the loop has already idled.
        pairing.note_idle_tick();
        pairing.note_idle_tick();
        assert_eq!(
            pairing.releasable_generation(9, 10),
            0,
            "held: the frame is still well inside the bound"
        );
        assert_eq!(
            pairing.releasable_generation(10, 10),
            u64::MAX,
            "the present releases it exactly as before"
        );
    }

    #[test]
    fn a_produced_batch_resets_the_idle_stretch() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 10);
        for _ in 0..(MAX_FRAMES_IN_FLIGHT - 1) {
            pairing.note_idle_tick();
        }
        // The loop wakes and paints again before the bound trips: frames are
        // flowing, so both batches are aged by the submission cursor alone.
        pairing.record(2, 11);
        assert_eq!(
            pairing.releasable_generation(9, 11),
            0,
            "a spent idle stretch cannot strand a live pipeline"
        );

        // Idling again ages them from scratch.
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            pairing.note_idle_tick();
        }
        assert_eq!(pairing.releasable_generation(9, 11), u64::MAX);
    }

    #[test]
    fn clear_drops_the_idle_stretch_with_the_pairings() {
        let mut pairing = FramePairing::new();
        pairing.record(1, 10);
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            pairing.note_idle_tick();
        }
        // Backgrounding / surface recreation: the ticks counted against a
        // session whose frames are gone must not age the next session's first
        // batch, which is gated normally.
        pairing.clear();
        pairing.record(2, 1);
        assert_eq!(pairing.releasable_generation(0, 1), 1);
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

    // ---- Z-shield auto-collection --------------------

    #[test]
    fn an_interactive_slot_carries_only_the_intersecting_shields() {
        let mut state = PlatformViewState::new();
        let slot = r(0.0, 0.0, 100.0, 100.0);
        let over = r(10.0, 10.0, 40.0, 40.0); // inside the slot
        let elsewhere = r(500.0, 500.0, 540.0, 540.0); // unrelated chrome

        state.ingest(&[interactive_frame(1, slot)], &[over, elsewhere]);
        assert_eq!(
            last_update_shields(&state),
            vec![over],
            "only the shield overlapping the slot rides its Update"
        );
    }

    #[test]
    fn a_non_interactive_slot_carries_no_shields_at_all() {
        let mut state = PlatformViewState::new();
        let slot = r(0.0, 0.0, 100.0, 100.0);
        // The same overlapping shield as above, but the slot forwards no input.
        state.ingest(&[frame(1, slot, true)], &[r(10.0, 10.0, 40.0, 40.0)]);
        assert!(
            last_update_shields(&state).is_empty(),
            "shields are meaningless to a host that isn't forwarding touches"
        );
    }

    #[test]
    fn a_moving_shield_emits_an_update_and_a_jittering_one_does_not() {
        let mut state = PlatformViewState::new();
        let slot = r(0.0, 0.0, 100.0, 100.0);
        state.ingest(&[interactive_frame(1, slot)], &[r(10.0, 10.0, 40.0, 40.0)]);
        state.acknowledge(state.commands().0);

        // Sub-epsilon drift: nothing (the same discipline as rect/clip).
        let changed = state.ingest(&[interactive_frame(1, slot)], &[r(10.2, 10.2, 40.2, 40.2)]);
        assert!(!changed, "sub-epsilon shield drift emits nothing");

        // A real move: an Update carrying the new shield, with the slot's own
        // rect unchanged.
        let moved = r(10.0, 60.0, 40.0, 90.0);
        let changed = state.ingest(&[interactive_frame(1, slot)], &[moved]);
        assert!(changed);
        assert_eq!(
            state.commands().1,
            &[ViewCommand::Update {
                slot_id: 1,
                rect: slot,
                clip: None,
                visible: true,
                shields: vec![moved],
                z_index: 0,
            }]
        );
    }

    #[test]
    fn a_shield_leaving_the_pass_clears_it_from_the_slot() {
        // The replace-per-pass contract end to end: chrome that stopped
        // painting must stop shielding, or it keeps stealing touches forever.
        let mut state = PlatformViewState::new();
        let slot = r(0.0, 0.0, 100.0, 100.0);
        state.ingest(&[interactive_frame(1, slot)], &[r(10.0, 10.0, 40.0, 40.0)]);
        state.acknowledge(state.commands().0);

        let changed = state.ingest(&[interactive_frame(1, slot)], &[]);
        assert!(changed);
        assert!(last_update_shields(&state).is_empty());
    }

    #[test]
    fn manual_shield_local_rects_are_kept_and_unioned_without_duplicates() {
        // The escape hatch still works, and a region declared BOTH ways lands
        // once.
        let mut state = PlatformViewState::new();
        let slot = r(0.0, 0.0, 100.0, 100.0);
        let manual = r(0.0, 0.0, 20.0, 20.0);
        let auto = r(50.0, 50.0, 70.0, 70.0);
        let mut f = interactive_frame(1, slot);
        f.shields = vec![manual];

        state.ingest(&[f], &[manual, auto]);
        assert_eq!(
            last_update_shields(&state),
            vec![manual, auto],
            "manual rects first, then the auto-collected ones, deduped"
        );
    }

    #[test]
    fn a_replay_preserves_each_slots_resolved_shields() {
        // Surface recreation must rebuild the native side from the replay
        // alone, shields included.
        let mut state = PlatformViewState::new();
        let slot = r(0.0, 0.0, 100.0, 100.0);
        let over = r(10.0, 10.0, 40.0, 40.0);
        state.ingest(&[interactive_frame(1, slot)], &[over]);
        state.acknowledge(state.commands().0);

        state.reset_for_surface_recreate();
        assert_eq!(last_update_shields(&state), vec![over]);
    }

    // ---- Widget teardown retire -------------------------

    #[test]
    fn a_retired_slot_disposes_immediately_and_the_next_ingest_is_quiet() {
        // The shell drains `RenderRoot::take_retired_platform_views()` after its
        // rebuild and retires each id; the paint that follows no longer
        // publishes the slot, and that absence must produce nothing further.
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);

        assert!(state.retire(1), "teardown disposes on the spot");
        assert_eq!(state.commands().1, &[ViewCommand::Dispose { slot_id: 1 }]);
        state.acknowledge(state.commands().0);

        let changed = state.ingest(&[], &[]);
        assert!(
            !changed,
            "the retired slot is already forgotten — no Hide, no second Dispose"
        );
    }

    #[test]
    fn a_merely_culled_slot_is_never_disposed_by_the_retire_path() {
        // The keep-alive contract: a scrolled-offscreen slot runs no
        // teardown, so no retire arrives; it is Hidden by the streak and stays
        // live well past the point a retire would have disposed it.
        let mut state = PlatformViewState::new();
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        state.acknowledge(state.commands().0);

        for _ in 0..(DISPOSE_AFTER_MISSING_FRAMES - 1) {
            state.ingest(&[], &[]);
        }
        let (_, cmds) = state.commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, ViewCommand::Update { visible: false, .. })),
            "the culled slot is hidden"
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, ViewCommand::Dispose { .. })),
            "but never disposed without a real teardown signal"
        );

        // It revives as a plain Update — the native view (and, for camera, the
        // session behind it) was never torn down.
        state.acknowledge(state.commands().0);
        state.ingest(&[frame(1, r(0.0, 0.0, 10.0, 10.0), true)], &[]);
        assert!(
            !state
                .commands()
                .1
                .iter()
                .any(|c| matches!(c, ViewCommand::Create { .. }))
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
                state.ingest(frames, &[]);
                all.extend(state.commands().1.iter().cloned());
                // Compact after observing, mirroring a real shell's poll+ack.
                state.acknowledge(state.commands().0);
            }
            all
        };

        assert_eq!(run(&sequence), run(&sequence));
    }

    // ---- z-order and overlays ---------------------------------------------

    /// The whole-window viewport every v1 overlay covers in these tests.
    const VIEWPORT: Rect = Rect::new(0.0, 0.0, 400.0, 800.0);

    /// A drawing command (a backdrop blur needs no brush, so the tests stay
    /// free of a `peniko` dependency).
    fn draw() -> Command {
        Command::BackdropBlur {
            rect: r(0.0, 0.0, 10.0, 10.0),
            radius: 4.0,
            transform: kurbo::Affine::IDENTITY,
        }
    }

    /// A Mode B slot's hole-punch, as `PaintScene::clear_rect` records it.
    fn punch() -> Command {
        Command::ClearRect {
            rect: r(0.0, 0.0, 100.0, 100.0),
            transform: kurbo::Affine::IDENTITY,
        }
    }

    fn push_clip() -> Command {
        Command::PushClip {
            rect: r(0.0, 0.0, 10.0, 10.0),
            transform: kurbo::Affine::IDENTITY,
        }
    }

    /// A slot frame published at painter-order index `command_index`.
    fn frame_at(slot_id: u64, command_index: usize) -> PlatformViewFrame {
        PlatformViewFrame {
            command_index,
            ..frame(slot_id, r(0.0, 0.0, 100.0, 100.0), true)
        }
    }

    /// Every `Overlay`/`DisposeOverlay` in the backlog, in order.
    fn overlay_commands(state: &PlatformViewState) -> Vec<ViewCommand> {
        state
            .commands()
            .1
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    ViewCommand::Overlay { .. } | ViewCommand::DisposeOverlay { .. }
                )
            })
            .cloned()
            .collect()
    }

    /// `(slot_id, z_index)` of every `Update` in the backlog, in order.
    fn update_z_indices(state: &PlatformViewState) -> Vec<(u64, u32)> {
        state
            .commands()
            .1
            .iter()
            .filter_map(|c| match c {
                ViewCommand::Update {
                    slot_id, z_index, ..
                } => Some((*slot_id, *z_index)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_slot_with_drawing_content_after_it_gets_one_overlay() {
        let mut state = PlatformViewState::new();
        // Backdrop, then the slot at index 1, then two drawn commands over it
        // (wrapped in a clip bracket).
        let commands = [draw(), push_clip(), draw(), draw(), Command::PopClip];
        assert!(state.ingest_scene(&[frame_at(1, 1)], &[], &commands, VIEWPORT));

        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 1..5,
            }]
        );
        assert_eq!(update_z_indices(&state), vec![(1, 0)]);
        // The overlay follows the slot's own Create + Update.
        assert!(matches!(
            state.commands().1.last(),
            Some(ViewCommand::Overlay { .. })
        ));
    }

    #[test]
    fn a_slots_own_punch_is_not_content_above_it() {
        // Mode B: the slot's punch is recorded at its own index. Alone, it
        // makes no overlay.
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), punch()], VIEWPORT);
        assert!(overlay_commands(&state).is_empty());

        // Real content after the punch does; the range still opens at the
        // slot's index, so the partition of the command list stays contiguous.
        let mut state = PlatformViewState::new();
        state.ingest_scene(
            &[frame_at(1, 1)],
            &[],
            &[draw(), punch(), push_clip(), draw(), Command::PopClip],
            VIEWPORT,
        );
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 1..5,
            }]
        );
    }

    #[test]
    fn structural_only_content_after_a_slot_gets_no_overlay() {
        let mut state = PlatformViewState::new();
        let commands = [
            draw(),
            push_clip(),
            Command::PopClip,
            Command::PushLayer {
                rect: r(0.0, 0.0, 10.0, 10.0),
                alpha: 0.5,
                transform: kurbo::Affine::IDENTITY,
            },
            Command::PopLayer,
        ];
        state.ingest_scene(&[frame_at(1, 1)], &[], &commands, VIEWPORT);
        assert!(overlay_commands(&state).is_empty());

        // Content painted only BEFORE the slot stays on the main surface.
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 3)], &[], &[draw(), draw(), draw()], VIEWPORT);
        assert!(overlay_commands(&state).is_empty());
    }

    #[test]
    fn two_slots_get_two_overlays_with_ranges_and_z_order() {
        let mut state = PlatformViewState::new();
        // draw | slot 1 @1 | draw draw | slot 2 @3 | push_clip draw pop
        let commands = [
            draw(),
            draw(),
            draw(),
            push_clip(),
            draw(),
            Command::PopClip,
        ];
        // Frames handed over out of painter order: z order follows
        // `command_index`, not the slice order.
        state.ingest_scene(&[frame_at(2, 3), frame_at(1, 1)], &[], &commands, VIEWPORT);

        assert_eq!(update_z_indices(&state), vec![(2, 1), (1, 0)]);
        assert_eq!(
            overlay_commands(&state),
            vec![
                ViewCommand::Overlay {
                    overlay_id: 1,
                    above_slot_id: 1,
                    rect: VIEWPORT,
                    command_range: 1..3,
                },
                ViewCommand::Overlay {
                    overlay_id: 2,
                    above_slot_id: 2,
                    rect: VIEWPORT,
                    command_range: 3..6,
                },
            ]
        );
    }

    #[test]
    fn content_only_between_two_slots_overlays_the_lower_one() {
        let mut state = PlatformViewState::new();
        // slot 1 @0 | draw | slot 2 @1 | (structural only) push_clip pop
        let commands = [draw(), push_clip(), Command::PopClip];
        state.ingest_scene(&[frame_at(1, 0), frame_at(2, 1)], &[], &commands, VIEWPORT);
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 0..1,
            }]
        );
    }

    #[test]
    fn a_hidden_slot_is_not_a_cut() {
        let mut state = PlatformViewState::new();
        // slot 1 @0 | draw | hidden slot 2 @1 | draw
        let mut hidden = frame_at(2, 1);
        hidden.visible = false;
        state.ingest_scene(&[frame_at(1, 0), hidden], &[], &[draw(), draw()], VIEWPORT);
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 0..2,
            }]
        );
    }

    #[test]
    fn an_unchanged_overlay_emits_nothing_and_a_moved_range_is_resent() {
        let mut state = PlatformViewState::new();
        let commands = [draw(), draw(), draw()];
        state.ingest_scene(&[frame_at(1, 1)], &[], &commands, VIEWPORT);
        state.acknowledge(state.commands().0);

        assert!(!state.ingest_scene(&[frame_at(1, 1)], &[], &commands, VIEWPORT));
        assert!(state.commands().1.is_empty());

        // One more command before the slot shifts its cut: same overlay id,
        // new range.
        let commands = [draw(), draw(), draw(), draw()];
        assert!(state.ingest_scene(&[frame_at(1, 2)], &[], &commands, VIEWPORT));
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 2..4,
            }]
        );
    }

    #[test]
    fn overlay_is_disposed_after_its_content_is_gone_for_the_dispose_streak() {
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        state.acknowledge(state.commands().0);

        // The content above the slot disappears: the overlay is emptied at
        // once (it must not render stale indices) but kept alive.
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw()], VIEWPORT);
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 1..1,
            }]
        );
        state.acknowledge(state.commands().0);

        for _ in 1..(DISPOSE_AFTER_MISSING_FRAMES - 1) {
            state.ingest_scene(&[frame_at(1, 1)], &[], &[draw()], VIEWPORT);
        }
        assert!(
            overlay_commands(&state).is_empty(),
            "nothing more until the dispose streak is reached"
        );

        assert!(state.ingest_scene(&[frame_at(1, 1)], &[], &[draw()], VIEWPORT));
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::DisposeOverlay { overlay_id: 1 }]
        );
        // The slot itself is untouched — it kept publishing all along.
        assert!(
            !state
                .commands()
                .1
                .iter()
                .any(|c| matches!(c, ViewCommand::Dispose { .. }))
        );

        // Content returning later gets a fresh overlay id.
        state.acknowledge(state.commands().0);
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 2,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 1..2,
            }]
        );
    }

    #[test]
    fn content_returning_before_disposal_reuses_the_overlay() {
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw()], VIEWPORT);
        state.acknowledge(state.commands().0);

        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        assert_eq!(
            overlay_commands(&state),
            vec![ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 1..2,
            }]
        );
    }

    #[test]
    fn a_frame_without_platform_views_emits_no_overlays() {
        let mut state = PlatformViewState::new();
        assert!(!state.ingest_scene(&[], &[], &[draw(), draw()], VIEWPORT));
        assert_eq!(state.commands(), (0, &[][..]));
    }

    #[test]
    fn plain_ingest_never_emits_overlays() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame_at(1, 1)], &[]);
        assert!(overlay_commands(&state).is_empty());
        assert_eq!(update_z_indices(&state), vec![(1, 0)]);
    }

    #[test]
    fn a_z_order_change_alone_emits_an_update() {
        let mut state = PlatformViewState::new();
        state.ingest(&[frame_at(1, 0), frame_at(2, 1)], &[]);
        state.acknowledge(state.commands().0);

        // Same geometry, swapped painter order.
        assert!(state.ingest(&[frame_at(1, 1), frame_at(2, 0)], &[]));
        assert_eq!(update_z_indices(&state), vec![(1, 1), (2, 0)]);
    }

    #[test]
    fn retiring_a_slot_disposes_its_overlay_first() {
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        state.acknowledge(state.commands().0);

        assert!(state.retire(1));
        assert_eq!(
            state.commands().1,
            &[
                ViewCommand::DisposeOverlay { overlay_id: 1 },
                ViewCommand::Dispose { slot_id: 1 },
            ]
        );
    }

    #[test]
    fn a_slot_disposed_by_its_missing_streak_takes_its_overlay_with_it() {
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        for _ in 0..DISPOSE_AFTER_MISSING_FRAMES {
            state.ingest_scene(&[], &[], &[draw()], VIEWPORT);
        }
        let cmds = state.commands().1;
        let dispose_overlay = cmds
            .iter()
            .position(|c| *c == ViewCommand::DisposeOverlay { overlay_id: 1 })
            .expect("the overlay is disposed");
        let dispose_slot = cmds
            .iter()
            .position(|c| *c == ViewCommand::Dispose { slot_id: 1 })
            .expect("the slot is disposed");
        assert!(dispose_overlay < dispose_slot);
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(c, ViewCommand::DisposeOverlay { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn surface_recreate_replays_live_overlays_after_the_slots() {
        let mut state = PlatformViewState::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        state.acknowledge(state.commands().0);

        assert!(state.reset_for_surface_recreate());
        let cmds = state.commands().1;
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[0], ViewCommand::Create { slot_id: 1, .. }));
        assert!(matches!(
            cmds[1],
            ViewCommand::Update {
                slot_id: 1,
                z_index: 0,
                ..
            }
        ));
        assert_eq!(
            cmds[2],
            ViewCommand::Overlay {
                overlay_id: 1,
                above_slot_id: 1,
                rect: VIEWPORT,
                command_range: 1..2,
            }
        );
    }

    #[test]
    fn overlays_ride_the_frame_pairing_release_gate() {
        let mut state = PlatformViewState::new();
        let mut pairing = FramePairing::new();
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw()], VIEWPORT);
        pairing.record(state.commands().0, 1);
        // Frame 2 adds content above the slot: its overlay is a new batch.
        state.ingest_scene(&[frame_at(1, 1)], &[], &[draw(), draw()], VIEWPORT);
        pairing.record(state.commands().0, 2);

        // Only frame 1 is on screen: the overlay is held with its frame.
        let (_, released) = state.commands_up_to(pairing.releasable_generation(1, 2));
        assert!(
            !released
                .iter()
                .any(|c| matches!(c, ViewCommand::Overlay { .. }))
        );
        // Frame 2 presents: the overlay goes out.
        let (_, released) = state.commands_up_to(pairing.releasable_generation(2, 2));
        assert!(
            released
                .iter()
                .any(|c| matches!(c, ViewCommand::Overlay { .. }))
        );
    }
}
