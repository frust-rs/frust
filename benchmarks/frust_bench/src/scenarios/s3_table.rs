//! S3 — Table ops (js-framework-benchmark subset).
//!
//! The classic jsfb sequence, run as a scripted, per-op-timed program: create
//! 1k rows, create 10k, update every 10th of the 10k, swap two rows, clear.
//! Each op is bracketed by its own sub-marker (`s3-create1k`, `s3-create10k`,
//! `s3-update`, `s3-swap`, `s3-clear`) so the harness times build/diff/
//! reconcile throughput per op — the frust side of the head-to-head against the
//! Flutter bench's `s3_table.dart`, whose op sequence, row labels, and
//! sub-marker names this file mirrors byte-for-byte (the fairness
//! gate; a "datasets.dart wins" rule does not reach here — S3's
//! op-sequence contract lives in `s3_table.dart`, mirrored here; the
//! continuous-cycling constant below is canonically homed in `datasets.dart`'s
//! `s3SettleGapMs`).
//!
//! # Continuous cycling (user-directed 2026-07-20)
//!
//! The op sequence CYCLES continuously for the whole capture window instead
//! of running once: the prior single-pass design legitimately emptied the
//! table at the terminal `clear` within the first few seconds, then sat blank
//! for the rest of a 30s capture (a real emptiness, not the "occlusion" bug
//! originally suspected — device data showed healthy per-op layouts
//! throughout). After `clear` closes and [`SETTLE_FRAMES`] elapses, the script
//! wraps back to [`Op::Create1k`] and repeats — forever, until the hosting
//! `S3Table` component itself is torn down (switching to another scenario).
//! No per-cycle RNG reseed is needed: S3 draws no randomness (each row's
//! label is a pure function of a monotonically-increasing `id`), and
//! [`Op::Create10k`] deterministically resets the id counter to 1 at the start
//! of every cycle — identically on both apps — so cycle N reproduces the
//! byte-identical row/label sequence cycle 1 did.
//!
//! # How the script is driven
//!
//! A [`Ticker`] widget requests a frame every paint (continuously — the
//! script never goes idle now that it cycles forever), so `S3Table::build`
//! re-runs each frame; the build advances a small per-op state machine. Each
//! op's window is exactly one reconcile frame wide: the build stamps
//! `mark_scenario_start(op)` and mutates the row set on the frame that op
//! starts (the mutation reconciles into the [`ListView`] during that frame's
//! layout/paint), then stamps `mark_scenario_end(op)` on the next frame, with
//! a short settle gap before the next op (or, after `clear`, before the next
//! cycle's `create1k`) so the per-op windows stay cleanly separable in the
//! trace — mirroring the Flutter side's
//! `mark → setState → await nextFrame → mark → settle-gap delay` shape.
//!
//! Rows render through the real virtualized `ListView` (the same
//! `ListView.builder` virtualization the Flutter side uses), so the reconcile
//! cost measured is the framework's own view-diff + arena rebuild over the
//! materialized window, not a hand-rolled painter.

use std::rc::Rc;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::{
    AnyView, Component, EdgeInsets, Padding, SizedBox, any, component, list_view, row, stack, text,
};

use super::{BenchState, Scenario};

/// S3 — Table ops.
pub struct S3;

impl Scenario for S3 {
    fn id(&self) -> &'static str {
        "s3"
    }

    fn title(&self) -> &'static str {
        "Table ops (create/update/swap/clear)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        // The scenario hosts a self-contained component owning the row set and
        // the script cursor; its `State` survives across `BenchState` rebuilds.
        any(component(S3Table))
    }
}

/// Row height in logical px — the uniform `ListView` item extent (matches the
/// Flutter side's `itemExtent: 40`).
const ROW_EXTENT: f64 = 40.0;

/// Frames to idle between two ops (or, after `clear`, before the next
/// cycle's `create1k`) so their trace windows don't overlap. ~300ms at 60fps,
/// matching the canonical `s3SettleGapMs` constant homed in the Flutter side's
/// `datasets.dart`.
const SETTLE_FRAMES: u32 = 18;

/// The jsfb label word banks — verbatim from the Flutter side's `s3_table.dart`
/// so a given `id` produces the byte-identical label on both apps.
const ADJECTIVES: [&str; 16] = [
    "pretty", "large", "big", "small", "tall", "short", "long", "handsome", "plain", "quaint",
    "clean", "elegant", "easy", "angry", "crazy", "helpful",
];
const COLOURS: [&str; 8] = [
    "red", "yellow", "blue", "green", "pink", "brown", "purple", "white",
];
const NOUNS: [&str; 13] = [
    "table", "chair", "house", "bbq", "desk", "car", "pony", "cookie", "sandwich", "burger",
    "pizza", "mouse", "keyboard",
];

/// The scripted ops, in run order — repeated continuously, cycle after
/// cycle (see the module doc's continuous-cycling contract), not run once.
const OPS: [Op; 5] = [Op::Create1k, Op::Create10k, Op::Update, Op::Swap, Op::Clear];

/// One scripted table op.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Create1k,
    Create10k,
    Update,
    Swap,
    Clear,
}

impl Op {
    /// The `s3-*` sub-marker name bracketing this op's reconcile frame.
    fn marker(self) -> &'static str {
        match self {
            Op::Create1k => "s3-create1k",
            Op::Create10k => "s3-create10k",
            Op::Update => "s3-update",
            Op::Swap => "s3-swap",
            Op::Clear => "s3-clear",
        }
    }
}

/// One table row — the classic jsfb `{id, label}`.
#[derive(Clone)]
struct TableRow {
    id: u64,
    label: String,
}

/// The label for `id`, cycling each word bank by `id` — verbatim jsfb math.
fn row_label(id: u64) -> String {
    format!(
        "{} {} {}",
        ADJECTIVES[(id as usize) % ADJECTIVES.len()],
        COLOURS[(id as usize) % COLOURS.len()],
        NOUNS[(id as usize) % NOUNS.len()],
    )
}

/// S3 — Table ops component.
pub struct S3Table;

/// Retained S3 state: the live row set (behind an `Rc` so the `ListView`
/// builder can cheaply capture a snapshot each frame), the next id to assign,
/// and the script cursor.
pub struct S3State {
    rows: Rc<Vec<TableRow>>,
    next_id: u64,
    /// Index into [`OPS`] of the op to run next within the current cycle.
    /// Wraps back to `0` immediately after the terminal `Clear` closes (see
    /// [`S3State::advance_script`]) — the script never sits at `OPS.len()`
    /// (that would mean "done", which S3 no longer ever is).
    op_index: usize,
    /// Frames left to idle before starting the next op (or, after `Clear`
    /// wraps, before the next cycle's `Create1k`).
    settle: u32,
    /// Whether the currently-open op's closing marker is still pending (its
    /// mutation reconciled last frame; close it this frame).
    awaiting_end: bool,
    /// Number of full op-sequence cycles completed so far (0 during the
    /// first cycle, incremented every time `Clear` closes and the script
    /// wraps back to `Create1k`) — bookkeeping/observability only; no
    /// dataset value depends on it (see the module doc's continuous-cycling
    /// contract for why no per-cycle reseed is needed).
    cycle: u32,
}

impl Component for S3Table {
    type State = S3State;

    fn init(&self) -> S3State {
        S3State {
            rows: Rc::new(Vec::new()),
            next_id: 1,
            op_index: 0,
            settle: 0,
            awaiting_end: false,
            cycle: 0,
        }
    }

    fn build(&self, state: &mut S3State) -> impl View<S3State> {
        state.advance_script();

        let rows = state.rows.clone();
        let count = rows.len();

        let table = list_view(count, ROW_EXTENT, move |i| {
            let item = &rows[i];
            any(Padding(
                EdgeInsets::symmetric(12.0, 4.0),
                row()
                    .child(text(item.id.to_string()).size(14.0))
                    .child(SizedBox(Some(12.0), None))
                    .flex(1, text(item.label.clone()).size(14.0)),
            ))
        });

        // The script cycles continuously for the whole capture window (see
        // the module doc), so the ticker stays active forever — it never
        // goes idle the way a run-once script would.
        any(stack().child(table).child(Ticker { active: true }))
    }
}

impl S3State {
    /// Advance the per-op state machine by one frame. Called once per
    /// rebuild. The script cycles forever: after `Clear`'s closing marker,
    /// `op_index` wraps back to `0` and `cycle` increments instead of the
    /// script going idle.
    fn advance_script(&mut self) {
        if self.awaiting_end {
            // The op mutated last frame and its change reconciled; close it.
            frust_shell_common::perf::mark_scenario_end(OPS[self.op_index].marker());
            self.awaiting_end = false;
            self.op_index += 1;
            if self.op_index >= OPS.len() {
                self.op_index = 0;
                self.cycle += 1;
            }
            self.settle = SETTLE_FRAMES;
            return;
        }

        if self.settle > 0 {
            self.settle -= 1;
            return;
        }

        // Start the next op: open its marker and apply the mutation, which
        // reconciles into the list during this same frame's layout/paint.
        let op = OPS[self.op_index];
        frust_shell_common::perf::mark_scenario_start(op.marker());
        self.apply(op);
        self.awaiting_end = true;
    }

    /// Apply one op's mutation to the row set.
    fn apply(&mut self, op: Op) {
        let rows = Rc::make_mut(&mut self.rows);
        match op {
            Op::Create1k => append(rows, &mut self.next_id, 1_000),
            Op::Create10k => {
                rows.clear();
                self.next_id = 1;
                append(rows, &mut self.next_id, 10_000);
            }
            Op::Update => {
                let mut i = 0;
                while i < rows.len() {
                    rows[i].label = format!("{} !!!", rows[i].label);
                    i += 10;
                }
            }
            Op::Swap => {
                if rows.len() >= 999 {
                    rows.swap(1, 998);
                }
            }
            Op::Clear => rows.clear(),
        }
    }
}

/// Append `count` freshly-id'd rows to `rows`.
fn append(rows: &mut Vec<TableRow>, next_id: &mut u64, count: usize) {
    rows.reserve(count);
    for _ in 0..count {
        let id = *next_id;
        *next_id += 1;
        rows.push(TableRow {
            id,
            label: row_label(id),
        });
    }
}

// ---------------------------------------------------------------------------
// Ticker — a zero-size paint-driven frame pump
// ---------------------------------------------------------------------------

/// A paint-only widget that requests a frame every paint while `active`, so the
/// hosting component rebuilds each frame and its script advances. `S3Table`
/// always passes `active: true` now that its script cycles continuously (see
/// the module doc) — `active: false` remains available for a future
/// run-once-then-idle consumer (desktop `ControlFlow::Wait`, mobile
/// frame-gate `Skip`), but S3 itself never goes idle.
pub struct Ticker {
    active: bool,
}

/// The retained ticker widget (see [`Ticker`]).
pub struct TickerWidget {
    active: bool,
}

impl<State: 'static> View<State> for Ticker {
    type Element = TickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TickerWidget {
        TickerWidget {
            active: self.active,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut TickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.active = self.active;
        ChangeFlags::PAINT
    }
}

impl Widget for TickerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
        Size::ZERO
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if self.active {
            ctx.request_frame();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_jsfb_word_banks() {
        // id 1 → adjectives[1] colours[1] nouns[1].
        assert_eq!(row_label(1), "large yellow chair");
        // id 0 → the first word of each bank.
        assert_eq!(row_label(0), "pretty red table");
    }

    #[test]
    fn script_cycles_continuously_clearing_and_repopulating() {
        // Regression/behavior lock for the continuous-cycling redesign: the
        // script no longer stops at `OPS.len()` (that state is now
        // unreachable — `advance_script` wraps `op_index` back to `0` in the
        // same call that closes the terminal `Clear`). Drive it for several
        // cycles' worth of frames and confirm the table clears AND
        // repopulates more than once, with the cycle counter advancing.
        let mut s = S3Table.init();
        let mut prev_empty = s.rows.is_empty(); // true initially (no rows yet)
        let mut empty_episodes = 0usize;
        let mut refills = 0usize;
        let mut max_cycle_seen = 0u32;
        for _ in 0..300 {
            s.advance_script();
            assert!(
                s.op_index < OPS.len(),
                "op_index must never sit at OPS.len() — it wraps in the same call that closes the terminal op"
            );
            max_cycle_seen = max_cycle_seen.max(s.cycle);
            let now_empty = s.rows.is_empty();
            if now_empty && !prev_empty {
                empty_episodes += 1;
            }
            if !now_empty && prev_empty {
                refills += 1;
            }
            prev_empty = now_empty;
        }
        assert!(
            empty_episodes >= 2,
            "the table must clear at the end of every cycle — at least twice \
             over this run window, got {empty_episodes}"
        );
        assert!(
            refills >= 2,
            "the table must repopulate after each cycle's clear — the next \
             cycle's create1k must refill the row set, got {refills}"
        );
        assert!(
            max_cycle_seen >= 2,
            "the cycle counter must advance past the first cycle over this \
             run window, got {max_cycle_seen}"
        );
    }

    #[test]
    fn update_touches_every_tenth_of_10k() {
        let mut s = S3Table.init();
        s.apply(Op::Create10k);
        s.apply(Op::Update);
        let bumped = s.rows.iter().filter(|r| r.label.ends_with("!!!")).count();
        assert_eq!(bumped, s.rows.len().div_ceil(10));
    }

    #[test]
    fn swap_exchanges_rows_1_and_998() {
        let mut s = S3Table.init();
        s.apply(Op::Create10k);
        let before_1 = s.rows[1].id;
        let before_998 = s.rows[998].id;
        s.apply(Op::Swap);
        assert_eq!(s.rows[1].id, before_998);
        assert_eq!(s.rows[998].id, before_1);
    }
}
