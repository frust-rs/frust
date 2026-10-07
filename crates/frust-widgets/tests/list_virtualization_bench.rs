//! Host-side virtualization perf evidence for `ListView`: the flat-vs-N claim
//! `list_view.rs`'s module docs make (*"Windowed materialization at rebuild
//! time"*) — a virtualized list's per-frame rebuild+layout cost is bounded by
//! the materialized *window*, not by `item_count`, while a naive
//! `ScrollView` + eagerly-built `Column` scales with `item_count` because it
//! reconstructs and reconciles every row on every frame.
//!
//! Two tests live here, deliberately split by what they can prove:
//!
//! - [`flat_vs_n_rebuild_layout_cost`] — `#[ignore]`d, host-machine
//!   dependent. Times 500 scripted-scroll frames per (config, N) cell over a
//!   3-configuration × 3-`N` grid and prints mean/p95/p99 per cell (per
//!   `docs/REVIEW_FOCUS.md`: a median-only perf claim is not a finding —
//!   tails are printed too). This is `docs/TESTING.md`'s T8
//!   (non-functional/performance) tier: scheduled/manual evidence, not a CI
//!   gate — `benchmarks/PROTOCOL.md` owns cross-framework device benchmarks;
//!   this is a narrower in-repo, host-only sanity instrument.
//!   Run: `cargo test -p frust-widgets --test list_virtualization_bench --
//!   --ignored --nocapture`.
//! - [`ten_thousand_rows_scrolled_midlist_materializes_only_the_window`] —
//!   always-on (no `#[ignore]`), CI-durable. Asserts the *structural* fact
//!   the timing numbers merely illustrate: at `N = 10_000` scrolled to the
//!   middle, both `ListView` configurations materialize an exact,
//!   hand-derived window (17 rows), while the `Column` baseline holds all
//!   10,000. No wall-clock assertion — host machines vary, structure
//!   doesn't.
//!
//! # Probe children
//!
//! Every row across both tests is a tiny hand-rolled [`ProbeView`]/
//! [`ProbeWidget`] pair (or, only for the baseline's structural build/rebuild
//! count, [`CountingProbeView`]/[`CountingProbeWidget`]) — paint-light (one
//! `fill_rect`) and deterministic, so the timed span isolates
//! `RenderRoot::rebuild`/`layout`/`paint`'s own materialization/reconciliation
//! cost rather than a real design-system widget's text shaping or paint
//! complexity.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene,
    RenderRoot, ScrollDelta, View, Widget, any,
};
use frust_widgets::{
    ChildKey, Column, ListView, ListViewWidget, ScrollView, list_view, scroll_view,
};
use kurbo::{Point, Size};

// --- Shared fixtures -------------------------------------------------------

/// The uniform row extent shared by every configuration (the `ListView`
/// uniform path's `item_extent`, the keyed path's `estimated_item_extent`,
/// and the baseline `Column`'s per-row height) — the same geometry across
/// configs is what makes the per-N comparison apples-to-apples.
const ITEM_EXTENT: f64 = 50.0;

/// The fixed viewport every configuration lays out and scrolls within.
fn viewport() -> Size {
    Size::new(320.0, 640.0)
}

/// A no-op paint sink — this suite only cares about frame *cost* and
/// materialized structure, never emitted draw commands (mirrors
/// `cull_pacing.rs`'s and `list_view.rs`'s own unit tests' `NullScene`).
struct NullScene;

impl PaintScene for NullScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
}

/// A paint-light, deterministic probe row: lays out to a fixed `height`
/// under whatever width it is given and paints one flat rect. Isolates the
/// timed span to `RenderRoot`'s own rebuild/layout/reconciliation cost
/// rather than a real widget's paint or text-shaping cost.
struct ProbeView {
    height: f64,
}

struct ProbeWidget {
    height: f64,
}

impl View<()> for ProbeView {
    type Element = ProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
        ProbeWidget {
            height: self.height,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.height != self.height {
            element.height = self.height;
            return ChangeFlags::LAYOUT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for ProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, self.height))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(
            Point::ORIGIN,
            Size::new(1.0, self.height),
            peniko::Color::from_rgb8(0x20, 0x78, 0xc8),
        );
    }
}

/// Deterministic, genuinely index-varying row height for the keyed
/// variable-extent config's *timing* cell — a small pseudo-varying pattern
/// (not literally random: `index * 37 mod 60`, offset to stay positive)
/// standing in for real variable-height rows (chat bubbles, wrapped text),
/// so the timed loop actually exercises `ListViewWidget`'s prefix-walk and
/// measured-extent cache instead of a degenerate case that happens to match
/// the estimate everywhere.
///
/// The *structural* test below deliberately does **not** use this — see its
/// own doc comment for why an exact window-size assertion needs a different,
/// estimate-matching height instead.
fn variable_height(index: usize) -> f64 {
    40.0 + ((index * 37) % 60) as f64
}

/// A deterministic ping-pong wheel-scroll pattern: `PING_ROWS` rows'
/// worth of pixels down, then the same distance back up, flipping every
/// `HALF_PERIOD` frames — enough churn to shift/relocate the materialized
/// window on most frames without depending on any config's `max_offset`
/// (every config hard-clamps wheel scrolling internally, see
/// `list_view.rs`/`scroll.rs`'s `InputEvent::Scroll` handling).
const PING_ROWS: f64 = 3.0;
const HALF_PERIOD: usize = 40;

fn scroll_delta_for(frame: usize) -> f64 {
    let dy = PING_ROWS * ITEM_EXTENT;
    if (frame / HALF_PERIOD).is_multiple_of(2) {
        dy
    } else {
        -dy
    }
}

/// Run one rebuild -> layout -> paint cycle at `ms`, mirroring the
/// rebuild/layout/paint shape `list_view.rs`'s own `#[cfg(test)] mod tests`
/// uses internally (its `frame` helper) — reused here in spelling only,
/// since an external `tests/` integration binary cannot see that private
/// helper.
fn drive_frame<V: View<()>>(
    root: &mut RenderRoot<(), V>,
    logic: &mut impl FnMut(&mut ()) -> V,
    state: &mut (),
    window: Size,
    ms: f64,
) {
    root.rebuild(logic, state);
    root.layout(window);
    let mut sink = NullScene;
    root.paint(&mut sink, FrameTime::from_nanos((ms * 1_000_000.0) as u64));
}

// --- (1) Ignored timing bench: flat-vs-N, with tails -----------------------

/// Drive `frames` scripted-scroll rebuild/layout/paint cycles against a
/// freshly built `ListView` root, returning one per-frame cost in
/// microseconds. The scroll event itself is dispatched *outside* the timed
/// span (input dispatch is O(1) offset arithmetic in every config — the
/// signal under test is materialization/reconciliation cost, not event
/// handling).
fn run_listview(frames: usize, mut make_view: impl FnMut() -> ListView<()>) -> Vec<f64> {
    let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
    let mut state = ();
    let win = viewport();
    let mut logic = move |_: &mut ()| make_view();

    // Two settle frames at offset 0, excluded from the timed loop: the first
    // materializes a conservative window against the not-yet-measured
    // viewport, the second converges to the real one — the same two-frame
    // shape `list_view.rs`'s `only_the_visible_window_materializes` unit test
    // documents.
    drive_frame(&mut root, &mut logic, &mut state, win, 0.0);
    drive_frame(&mut root, &mut logic, &mut state, win, 16.0);

    let mut samples = Vec::with_capacity(frames);
    let mut ms = 32.0;
    for f in 0..frames {
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, win.height / 2.0),
                delta: ScrollDelta::Pixels(0.0, scroll_delta_for(f)),
            },
        );
        let start = Instant::now();
        drive_frame(&mut root, &mut logic, &mut state, win, ms);
        samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
        ms += 16.0;
    }
    samples
}

/// The `ScrollView` + eagerly-built `Column` baseline: every frame's
/// build closure reconstructs a fresh `Vec` of `n` `AnyView`s (no windowing),
/// so `Flex`'s positional reconciliation diffs all `n` children every frame
/// — the O(N) counterpart to [`run_listview`]'s O(window).
fn run_baseline(n: usize, frames: usize) -> Vec<f64> {
    let mut root: RenderRoot<(), ScrollView<()>> = RenderRoot::new();
    let mut state = ();
    let win = viewport();
    let mut logic = move |_: &mut ()| -> ScrollView<()> {
        scroll_view(Column(
            (0..n)
                .map(|_| {
                    any::<(), _>(ProbeView {
                        height: ITEM_EXTENT,
                    })
                })
                .collect::<Vec<_>>(),
        ))
    };

    drive_frame(&mut root, &mut logic, &mut state, win, 0.0);
    drive_frame(&mut root, &mut logic, &mut state, win, 16.0);

    let mut samples = Vec::with_capacity(frames);
    let mut ms = 32.0;
    for f in 0..frames {
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(10.0, win.height / 2.0),
                delta: ScrollDelta::Pixels(0.0, scroll_delta_for(f)),
            },
        );
        let start = Instant::now();
        drive_frame(&mut root, &mut logic, &mut state, win, ms);
        samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
        ms += 16.0;
    }
    samples
}

/// Mean/p95/p99 over one (config, N) cell's per-frame microsecond samples.
struct Stats {
    mean: f64,
    p95: f64,
    p99: f64,
}

fn stats(mut samples: Vec<f64>) -> Stats {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("frame timings are always finite"));
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    Stats {
        mean,
        p95: percentile(&samples, 0.95),
        p99: percentile(&samples, 0.99),
    }
}

/// Nearest-rank percentile over an already-sorted sample set.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let rank = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn print_row(config: &str, n: usize, s: Stats) {
    println!(
        "{config:<28} {n:>8} {:>12.2} {:>12.2} {:>12.2}",
        s.mean, s.p95, s.p99
    );
}

#[test]
#[ignore = "perf evidence, host-machine dependent (docs/TESTING.md T8): run via \
            `cargo test -p frust-widgets --test list_virtualization_bench -- --ignored --nocapture`"]
fn flat_vs_n_rebuild_layout_cost() {
    const FRAMES: usize = 500;
    const NS: [usize; 3] = [100, 1_000, 10_000];

    println!(
        "\n{:<28} {:>8} {:>12} {:>12} {:>12}",
        "config", "N", "mean_us", "p95_us", "p99_us"
    );

    for &n in &NS {
        let samples = run_listview(FRAMES, move || {
            list_view(n, ITEM_EXTENT, |_i| {
                any::<(), _>(ProbeView {
                    height: ITEM_EXTENT,
                })
            })
        });
        print_row("listview_uniform", n, stats(samples));
    }

    for &n in &NS {
        let samples = run_listview(FRAMES, move || {
            ListView::builder_keyed(n, ITEM_EXTENT, ChildKey::new, |i| {
                any::<(), _>(ProbeView {
                    height: variable_height(i),
                })
            })
            .estimated_item_extent(ITEM_EXTENT)
        });
        print_row("listview_keyed_variable", n, stats(samples));
    }

    for &n in &NS {
        let samples = run_baseline(n, FRAMES);
        print_row("scrollview_column_baseline", n, stats(samples));
    }

    println!(
        "\nExpected shape: listview_* mean/p95/p99 stay ~flat from N=1_000 to \
         N=10_000; scrollview_column_baseline scales ~linearly with N (see this \
         file's module docs and the task's completion summary for the recorded run)."
    );
}

// --- (2) Always-on structural sanity: O(window), not O(N) ------------------

/// Downcast the `RenderRoot`'s root widget to `&ListViewWidget` — the same
/// `Widget: Any` trait-upcast `list_view.rs`'s own `#[cfg(test)]` helper
/// (`list_widget`) uses, reimplemented here since an external `tests/`
/// binary cannot see that private helper, only the public `ListViewWidget`
/// type and its public accessors (`window()`, `offset()`).
fn list_view_widget(root: &RenderRoot<(), ListView<()>>) -> &ListViewWidget {
    let id = root.root_id().expect("root built");
    (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
        .downcast_ref::<ListViewWidget>()
        .expect("root is a ListViewWidget")
}

/// Build `logic`'s `ListView`, settle it at offset 0 (two frames — see
/// [`run_listview`]'s comment for why two), scroll it to `target_offset` in
/// one wheel event, run one more frame, and return the materialized window
/// (item indices).
fn scrolled_window(
    mut logic: impl FnMut(&mut ()) -> ListView<()>,
    win: Size,
    target_offset: f64,
) -> Vec<usize> {
    let mut root: RenderRoot<(), ListView<()>> = RenderRoot::new();
    let mut state = ();
    drive_frame(&mut root, &mut logic, &mut state, win, 0.0);
    drive_frame(&mut root, &mut logic, &mut state, win, 16.0);
    root.event(
        &mut state,
        &InputEvent::Scroll {
            position: Point::new(10.0, win.height / 2.0),
            delta: ScrollDelta::Pixels(0.0, target_offset),
        },
    );
    drive_frame(&mut root, &mut logic, &mut state, win, 32.0);

    let w = list_view_widget(&root);
    assert_eq!(
        w.offset(),
        target_offset,
        "the target offset is well inside [0, max_offset] for N=10_000, so the \
         single wheel event lands exactly on it with no clamping"
    );
    w.window().to_vec()
}

/// A probe that also counts `build`/`rebuild` calls — used only by the
/// baseline's structural assertion below, to prove "the Column holds N
/// widgets" as a fact about the *retained* tree (every one of the N rows is
/// both built on the first frame and revisited on the next) rather than
/// reaching into `FlexWidget`'s private `children` field, which this crate
/// does not expose to an external `tests/` binary.
struct CountingProbeView {
    height: f64,
    built: Rc<Cell<u32>>,
    rebuilt: Rc<Cell<u32>>,
}

struct CountingProbeWidget {
    height: f64,
}

impl View<()> for CountingProbeView {
    type Element = CountingProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CountingProbeWidget {
        self.built.set(self.built.get() + 1);
        CountingProbeWidget {
            height: self.height,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut CountingProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.rebuilt.set(self.rebuilt.get() + 1);
        element.height = self.height;
        ChangeFlags::NONE
    }
}

impl Widget for CountingProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(bc.max().width, self.height))
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

/// The structural claim PLAN.md's flat-vs-N evidence rests on, made
/// CI-durable (`docs/REVIEW_FOCUS.md`/`docs/TESTING.md`'s "no wall-clock
/// assertions in a gated test" line): at `N = 10_000` scrolled to the
/// middle, both `ListView` configurations materialize an *exact*,
/// hand-derived window rather than something merely "small"; the `Column`
/// baseline holds all 10,000.
///
/// # Deriving the expected window
///
/// `TARGET_OFFSET = 5_000 * ITEM_EXTENT = 250_000.0` — an exact multiple of
/// `ITEM_EXTENT`, so both the row index and every boundary below land on a
/// whole row with no fractional-pixel rounding to reason about.
///
/// `ListViewWidget::desired_window`'s uniform closed form (`list_view.rs`,
/// private `BUFFER = 2`) is `floor(offset/extent) − BUFFER .. ceil((offset +
/// viewport.height)/extent) + BUFFER`:
///
/// ```text
/// floor(250_000 / 50) − 2        = 5_000 − 2 = 4_998
/// ceil((250_000 + 640) / 50) + 2 = ceil(5_012.8) + 2 = 5_013 + 2 = 5_015
/// ```
///
/// giving `[4_998, 5_015)` — **17 rows**.
///
/// The keyed variable-extent config below deliberately uses a row height
/// that exactly equals its `estimated_item_extent` (`ITEM_EXTENT`, *not*
/// [`variable_height`]) — every `extent_at` lookup in
/// `ListViewWidget::walk_to_offset`/`desired_window_variable` (measured or
/// still-estimated) then returns the same constant, which collapses the
/// prefix-walk arithmetic to *exactly* the closed form above: the same
/// jump-then-walk math, over a constant step, computes the identical
/// `[4_998, 5_015)`. Hand-verified by walking `walk_to_offset` from a cold
/// anchor (`anchor_index = 0, anchor_y = 0.0`, the widget's build-time
/// default): the `MAX_PREFIX_STEP`-gated jump lands exactly on index 5_000 at
/// `y = 250_000.0`, and the forward/backward buffer walks land on the same
/// boundaries as the uniform path. A genuinely varying height (as
/// [`variable_height`] gives the timing bench, above) is real but not
/// hand-solvable in closed form without depending on the exact multi-frame
/// convergence history — using it here would trade an exact, provable
/// assertion for an approximate one; the timing bench already covers that
/// code path's *cost*, this test only needs its *materialization count*.
#[test]
fn ten_thousand_rows_scrolled_midlist_materializes_only_the_window() {
    const N: usize = 10_000;
    const TARGET_OFFSET: f64 = 5_000.0 * ITEM_EXTENT;
    const EXPECTED_START: usize = 4_998;
    const EXPECTED_END: usize = 5_015;

    let win = viewport();
    let expected: Vec<usize> = (EXPECTED_START..EXPECTED_END).collect();
    assert_eq!(
        expected.len(),
        17,
        "sanity-check the hand derivation itself"
    );

    // --- (a) uniform: the closed form directly. ---
    let uniform_window = scrolled_window(
        move |_: &mut ()| {
            list_view(N, ITEM_EXTENT, |_i| {
                any::<(), _>(ProbeView {
                    height: ITEM_EXTENT,
                })
            })
        },
        win,
        TARGET_OFFSET,
    );
    assert_eq!(
        uniform_window, expected,
        "uniform ListView must materialize exactly the closed-form window \
         [{EXPECTED_START}, {EXPECTED_END}) at N={N} scrolled mid-list, never \
         the full {N} rows"
    );

    // --- (b) keyed, variable-extent mode, height == estimate (see the test's
    //     doc comment for why this still exercises the real variable-extent
    //     code path while staying exactly predictable). ---
    let keyed_window = scrolled_window(
        move |_: &mut ()| {
            ListView::builder_keyed(N, ITEM_EXTENT, ChildKey::new, |_i| {
                any::<(), _>(ProbeView {
                    height: ITEM_EXTENT,
                })
            })
            .estimated_item_extent(ITEM_EXTENT)
        },
        win,
        TARGET_OFFSET,
    );
    assert_eq!(
        keyed_window, expected,
        "keyed variable-extent ListView (height == estimate) must materialize \
         the identical closed-form window as the uniform path"
    );

    // --- (c) baseline: the Column holds every one of the N rows, always. ---
    let built = Rc::new(Cell::new(0u32));
    let rebuilt = Rc::new(Cell::new(0u32));
    let built_for_logic = built.clone();
    let rebuilt_for_logic = rebuilt.clone();
    let mut root: RenderRoot<(), ScrollView<()>> = RenderRoot::new();
    let mut state = ();
    let mut logic = move |_: &mut ()| -> ScrollView<()> {
        scroll_view(Column(
            (0..N)
                .map(|_| {
                    any::<(), _>(CountingProbeView {
                        height: ITEM_EXTENT,
                        built: built_for_logic.clone(),
                        rebuilt: rebuilt_for_logic.clone(),
                    })
                })
                .collect::<Vec<_>>(),
        ))
    };
    drive_frame(&mut root, &mut logic, &mut state, win, 0.0);
    assert_eq!(
        built.get() as usize,
        N,
        "the Column baseline eagerly builds every one of the N rows on the \
         very first frame, never a windowed subset"
    );
    drive_frame(&mut root, &mut logic, &mut state, win, 16.0);
    assert_eq!(
        rebuilt.get() as usize,
        N,
        "and every one of the N rows is reconciled again on the very next \
         frame too — it holds all N widgets, not a materialized window"
    );
    assert_eq!(
        built.get() as usize,
        N,
        "no additional builds on the second frame: the same N widgets are \
         retained (not virtualized away and rebuilt), just all N reconciled \
         every pass"
    );
}
