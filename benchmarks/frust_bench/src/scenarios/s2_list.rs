//! S2 — Long-list scroll (10k rows, scripted fling + steady scroll).
//!
//! # Why a hand-rolled list instead of `frust::ListView`
//!
//! `frust-widgets::ListView` virtualizes a row window,
//! but its scroll offset is driven **only** by real `InputEvent`s routed
//! through `RenderRoot::event` (drag/wheel) — there is no programmatic
//! "scroll-to"/controller API, and app code (this scenario) has no access to
//! the shell's event-dispatch seam to synthesize one. A `ListView` fling is
//! itself advanced during paint with no `EventCtx`, so even its own
//! `on_near_start`/`on_near_end` callbacks only *fire* on the next real event
//! (see its module docs' "pending" delivery) — meaning a scenario with no
//! live mouse/touch input would never see a pagination callback land.
//!
//! Per this task's explicit permission ("driven by synthetic input **or an
//! auto-scroll driver**"), S2 is instead a self-contained `View`/`Widget` pair
//! (the same low-level `frust-core`/`kurbo`/`peniko` escape hatch S1's
//! `BubbleChart` uses) that:
//!
//! - drives its own scroll offset from a fixed, deterministic scripted
//!   timeline advanced during **paint** (`PaintCtx::frame_time`), exactly like
//!   `BubbleChartWidget` advances its physics, and **publishes that offset back
//!   through a signal** so the next rebuild reports `ChangeFlags::LAYOUT` and
//!   the row window is re-laid-out as the scroll advances — the same
//!   force-a-relayout-from-paint contract S6's `WidthBox` uses. This matters on
//!   Android specifically: its frame gate skips layout after the first frame
//!   unless the rebuild reports `needs_layout` (`docs/ARCHITECTURE.md`'s Frame
//!   gate), and the materialized row window (plus the shaped-text cache, which
//!   needs the layout-time `TextContext`) is computed only during layout — so a
//!   scroll that advances only during paint, reporting only `PAINT`, would
//!   freeze the window at offset 0 and slide every row off-screen into an empty
//!   scene. The script itself is a steady scroll to 25%, a fast
//!   ease-out fling to the end, a steady scroll back to the top, looping
//!   (Flutter's own S2 script runs this sequence once; looping it here keeps
//!   the scenario exercising continuously for however long the harness
//!   measures it — a deliberate, documented Frust-side divergence in
//!   *duration* only, not in the class of workload);
//! - virtualizes its own row window (the same windowed-materialization idea
//!   `ListView` uses, reimplemented locally since we don't have access to its
//!   crate-private `build_child`/`rebuild_child` helpers from outside
//!   `frust-widgets`) using `frust`'s public `text`/`icon` leaf widgets, built
//!   and driven directly via `frust_core`'s public `BuildCtx`/`LayoutCtx`/
//!   `PaintCtx`/`Widget` API (the same pattern `frust-widgets`' own
//!   containers use internally, available here since it's all public API);
//! - reports a "nearing the not-yet-loaded edge" condition back to a nested
//!   [`frust::Component`] via an `RwSignal<bool>` written during paint (the
//!   same signal-write-during-paint idiom `BubbleChartWidget` uses for its FPS
//!   readout) rather than `ListView`'s event-routed `on_near_end` callback,
//!   since paint carries no `EventCtx`/application state to fire a callback
//!   with (`docs/ARCHITECTURE.md`'s Event pipeline: "Event-pass code never
//!   reads a theme — EventCtx carries none" — paint carries even less: no
//!   state at all). The nested component's `build` (which *does* get direct
//!   `&mut State` access every rebuild, no `EventCtx` required) reads that
//!   signal and drives the `use_task`-based pagination.
//!
//! Row content — the thumbnail hue (`(index * 137) % 360`, HSV(0.55, 0.85)),
//! title/subtitle strings, and the cycling trailing-icon set — mirrors the
//! Flutter side's `s2_list.dart` `_ListRow` shape for visual parity; this is
//! **not** a byte-identical dataset-parity requirement (`datasets.dart` has no
//! S2 section — only S4/S5 are bound by the ADDENDUM's cross-app parity
//! contract), since every value here is a pure function of the row index that
//! both apps can reproduce independently with no shared PRNG stream.
//!
//! # Pagination
//!
//! The list starts with [`INITIAL_LOADED`] of the [`TOTAL_ROWS`] rows
//! "available"; rows beyond the loaded edge paint a "Loading…" placeholder.
//! Nearing that edge (within [`NEAR_END_MARGIN_PX`]) triggers a simulated
//! paginated fetch — `use_task` + `frust::spawn_blocking` sleeping
//! [`FETCH_LATENCY_MS`] to stand in for network latency — that grows the
//! loaded count by [`BATCH_SIZE`] once it resolves, capped at
//! [`TOTAL_ROWS`]. This is the scenario's `on_near_start`/`on_near_end`-style
//! pagination requirement, implemented over the signal channel described
//! above instead of `ListView`'s own callback.

use std::collections::HashMap;
use std::time::Duration;

use frust::{
    AnyView, Color, Component, Curve, FrameTime, Get, IconSource, RwSignal, Set, UseTask, any,
    component, icon, icons, spawn_blocking, text, use_task,
};
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Point, Size};

use super::{BenchState, Scenario};

/// Total dataset size.
const TOTAL_ROWS: usize = 10_000;
/// Row height, matching the Flutter side's `itemExtent: 72`.
const ITEM_EXTENT: f64 = 72.0;
/// Thumbnail square side length.
const THUMB_SIZE: f64 = 48.0;
/// Trailing icon side length.
const ICON_SIZE: f64 = 22.0;
/// Horizontal padding between row elements.
const ROW_PAD: f64 = 12.0;
/// Extra rows materialized above/below the visible viewport.
const BUFFER_ROWS: isize = 3;
/// Extra rows kept in the shaped-text cache beyond the materialized window,
/// so a small scroll doesn't immediately evict/reshape a just-off-window row.
const CACHE_MARGIN_ROWS: usize = 12;

/// Rows "available" at start, before the first simulated pagination fetch
/// resolves.
const INITIAL_LOADED: usize = 2_000;
/// Rows a single simulated fetch adds once it resolves.
const BATCH_SIZE: usize = 2_000;
/// Simulated network latency for one pagination fetch.
const FETCH_LATENCY_MS: u64 = 120;
/// How close (in px) the scrolled viewport's trailing edge must come to the
/// loaded edge before a pagination fetch is triggered.
const NEAR_END_MARGIN_PX: f64 = ITEM_EXTENT * 8.0;

/// Scripted-scroll timeline phases (steady scroll to 25%, fast ease-out fling
/// to the end, steady scroll back — mirrors the Flutter side's `s2_list.dart`
/// `_runScript`, looped continuously here; see the module doc above).
const SCRIPT_PHASE1_MS: f64 = 4_000.0;
const SCRIPT_PHASE2_MS: f64 = 900.0;
const SCRIPT_PHASE3_MS: f64 = 6_000.0;
const SCRIPT_CYCLE_MS: f64 = SCRIPT_PHASE1_MS + SCRIPT_PHASE2_MS + SCRIPT_PHASE3_MS;

const BACKGROUND: Color = Color::from_rgb8(0x10, 0x12, 0x16);
const TITLE_COLOR: Color = Color::from_rgb8(0xF2, 0xF2, 0xF2);
const SUBTITLE_COLOR: Color = Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xB0);
const LOADING_COLOR: Color = Color::from_rgba8(0xFF, 0xFF, 0xFF, 0x90);
const ICON_COLOR: Color = Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xCC);

/// The trailing icon set the row cycles through (mirrors the Flutter repro's
/// `_trailingIcons` list: star / favorite / bookmark / flag / check_circle).
const TRAILING_ICONS: [IconSource; 5] = [
    icons::STAR,
    icons::PUSH_PIN,
    icons::ARCHIVE,
    icons::CHECK,
    icons::STAR_FILLED,
];

/// S2 — Long-list scroll.
pub struct S2;

impl Scenario for S2 {
    fn id(&self) -> &'static str {
        "s2"
    }

    fn title(&self) -> &'static str {
        "Long-list scroll (10k rows)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(LongListPage))
    }
}

// --- The nested component: retained pagination state, invisible to `BenchState` ---

struct LongListPage;

/// [`LongListPage`]'s retained local state (the `Component::State` model) —
/// deliberately NOT part of the shared `BenchState` (kept local to this file
/// only), scoped instead to this component's own reactive [`frust::Owner`].
struct LongListState {
    /// How many of [`TOTAL_ROWS`] rows are currently "available".
    loaded: usize,
    /// Whether a simulated pagination fetch is in flight.
    fetching: bool,
    task: UseTask<usize>,
    /// Written by the list widget during paint: whether the scrolled viewport
    /// is nearing the loaded edge (see the module doc's Pagination section).
    near_end: RwSignal<bool>,
    /// The scripted scroll offset, published by the list widget during paint
    /// and read back (tracked) here every rebuild. Threading the offset through
    /// a signal — rather than keeping it purely widget-internal — is what lets
    /// [`LongList::rebuild`] report `ChangeFlags::LAYOUT` when the scroll
    /// advances, forcing the materialized row window to be re-laid-out (and new
    /// rows shaped) on every platform, including Android's otherwise
    /// layout-skipping frame gate (see `docs/ARCHITECTURE.md`'s Frame gate and
    /// the S6 `WidthBox` precedent). Without it, layout is skipped after frame 1
    /// on device, the window freezes at offset 0, and the scripted scroll slides
    /// every materialized row off-screen into an empty scene.
    offset: RwSignal<f64>,
}

impl Component for LongListPage {
    type State = LongListState;

    fn init(&self) -> LongListState {
        let task = use_task(|| async {
            spawn_blocking(|| {
                std::thread::sleep(Duration::from_millis(FETCH_LATENCY_MS));
                BATCH_SIZE
            })
            .await
        });
        LongListState {
            loaded: INITIAL_LOADED,
            fetching: true,
            task,
            near_end: RwSignal::new(false),
            offset: RwSignal::new(0.0),
        }
    }

    fn build(&self, state: &mut LongListState) -> AnyView<LongListState> {
        if state.fetching {
            let current = state.task.signal().get();
            if let Some(&added) = current.ready() {
                state.loaded = (state.loaded + added).min(TOTAL_ROWS);
                state.fetching = false;
            }
        }
        if !state.fetching && state.loaded < TOTAL_ROWS && state.near_end.get() {
            state.fetching = true;
            state.task.restart();
        }
        any(LongList {
            loaded: state.loaded,
            near_end: state.near_end,
            offset: state.offset,
            // Tracked read: a paint-time `offset.set` wakes the next rebuild,
            // whose `LongList::rebuild` then reports `ChangeFlags::LAYOUT` (see
            // `LongListState::offset`).
            scroll_offset: state.offset.get(),
        })
    }
}

// --- The hand-rolled virtualized list view/widget (see module doc) ---

struct LongList {
    loaded: usize,
    near_end: RwSignal<bool>,
    /// The scroll-offset signal the widget publishes to during paint.
    offset: RwSignal<f64>,
    /// The current value of [`offset`](Self::offset), read (tracked) in the
    /// component build; a change between rebuilds is what triggers the
    /// `ChangeFlags::LAYOUT` that re-windows the list under the Android gate.
    scroll_offset: f64,
}

/// A visible row's pre-built leaf widgets, cached by row index across frames
/// (rebuilt only when the row leaves and re-enters the materialized window).
struct RowVisual {
    title: Box<dyn Widget>,
    subtitle: Box<dyn Widget>,
}

struct LongListWidget {
    loaded: usize,
    near_end: RwSignal<bool>,
    last_near_end_reported: bool,
    /// The scroll-offset signal published during paint (see [`LongList::offset`]).
    offset_signal: RwSignal<f64>,
    /// The last offset value published to [`offset_signal`](Self::offset_signal),
    /// so a paint only writes the signal (waking a relayout-forcing rebuild) when
    /// the scripted scroll actually advanced.
    last_offset_published: f64,
    offset: f64,
    viewport: Size,
    window: (usize, usize),
    script_ms: f64,
    last_frame: Option<FrameTime>,
    cache: HashMap<usize, RowVisual>,
    icon_widgets: Vec<Box<dyn Widget>>,
    loading_label: Option<Box<dyn Widget>>,
}

/// Build `view` as a boxed [`Widget`], pinning its `View::State` to `()` — the
/// leaf widgets this scenario reuses (`text`/`icon`) never touch application
/// state, so any state type would do; `()` documents that at the call site.
fn build_leaf<V>(view: V) -> Box<dyn Widget>
where
    V: View<()>,
{
    let mut next_id = 0u64;
    let mut ctx = BuildCtx::new(&mut next_id);
    Box::new(<V as View<()>>::build(&view, &mut ctx))
}

/// Shape a row's title/subtitle text once (cached by index — see
/// [`LongListWidget::cache`]).
fn build_row_visual(ctx: &mut LayoutCtx, index: usize) -> RowVisual {
    let title = text(format!("Row {index} — long-list scroll benchmark"))
        .size(15.0)
        .color(TITLE_COLOR);
    let subtitle = text(format!("Deterministic content for row number {index}"))
        .size(12.0)
        .color(SUBTITLE_COLOR);
    let mut title_w = build_leaf(title);
    let mut subtitle_w = build_leaf(subtitle);
    let bc = BoxConstraints::loose(Size::new(f64::INFINITY, ITEM_EXTENT));
    title_w.layout(ctx, &bc);
    subtitle_w.layout(ctx, &bc);
    RowVisual {
        title: title_w,
        subtitle: subtitle_w,
    }
}

/// HSV → RGB (standard algorithm, matching the Flutter side's
/// `HSVColor.fromAHSV(1.0, hue, 0.55, 0.85).toColor()` closely enough for
/// visual parity — see the module doc's note on why this isn't a
/// byte-identical dataset-parity requirement).
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as i64 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        (((r1 + m).clamp(0.0, 1.0)) * 255.0).round() as u8,
        (((g1 + m).clamp(0.0, 1.0)) * 255.0).round() as u8,
        (((b1 + m).clamp(0.0, 1.0)) * 255.0).round() as u8,
    )
}

/// The scripted scroll offset at elapsed time `t_ms` within one
/// [`SCRIPT_CYCLE_MS`] cycle (see the module doc's phase breakdown).
fn scripted_offset(t_ms: f64, max_offset: f64) -> f64 {
    if max_offset <= 0.0 {
        return 0.0;
    }
    let quarter = max_offset * 0.25;
    if t_ms < SCRIPT_PHASE1_MS {
        let f = (t_ms / SCRIPT_PHASE1_MS).clamp(0.0, 1.0);
        Curve::Linear.transform(f) * quarter
    } else if t_ms < SCRIPT_PHASE1_MS + SCRIPT_PHASE2_MS {
        let f = ((t_ms - SCRIPT_PHASE1_MS) / SCRIPT_PHASE2_MS).clamp(0.0, 1.0);
        let eased = Curve::EaseOut.transform(f);
        quarter + eased * (max_offset - quarter)
    } else {
        let f = ((t_ms - SCRIPT_PHASE1_MS - SCRIPT_PHASE2_MS) / SCRIPT_PHASE3_MS).clamp(0.0, 1.0);
        max_offset - Curve::Linear.transform(f) * max_offset
    }
}

impl LongListWidget {
    fn new(loaded: usize, near_end: RwSignal<bool>, offset_signal: RwSignal<f64>) -> Self {
        let icon_widgets: Vec<Box<dyn Widget>> = TRAILING_ICONS
            .into_iter()
            .map(|source| {
                let mut w = build_leaf(icon(source).size(ICON_SIZE).color(ICON_COLOR));
                w.layout(
                    &mut LayoutCtx::new(),
                    &BoxConstraints::tight(Size::new(ICON_SIZE, ICON_SIZE)),
                );
                w
            })
            .collect();
        Self {
            loaded,
            near_end,
            last_near_end_reported: false,
            offset_signal,
            last_offset_published: 0.0,
            offset: 0.0,
            viewport: Size::ZERO,
            window: (0, 0),
            script_ms: 0.0,
            last_frame: None,
            cache: HashMap::new(),
            icon_widgets,
            loading_label: None,
        }
    }

    fn max_offset(&self) -> f64 {
        (TOTAL_ROWS as f64 * ITEM_EXTENT - self.viewport.height).max(0.0)
    }

    /// The `[start, end)` row range that should be materialized for the
    /// current offset + cached viewport (mirrors
    /// `frust-widgets::ListView`'s own windowing math).
    fn desired_window(&self) -> (usize, usize) {
        if self.viewport.height <= 0.0 {
            return (0, 0);
        }
        let first = (self.offset / ITEM_EXTENT).floor() as isize - BUFFER_ROWS;
        let last =
            ((self.offset + self.viewport.height) / ITEM_EXTENT).ceil() as isize + BUFFER_ROWS;
        let start = first.max(0) as usize;
        let end = (last.max(0) as usize).min(TOTAL_ROWS);
        (start.min(end), end)
    }

    fn advance_script(&mut self, now: FrameTime) {
        let dt_ms = match self.last_frame {
            Some(prev) => now.saturating_sub(prev).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        self.last_frame = Some(now);
        self.script_ms = (self.script_ms + dt_ms) % SCRIPT_CYCLE_MS;
        // Publish (only on change) the next scripted offset through the signal
        // rather than mutating the drawn offset directly. The drawn/windowed
        // `self.offset` is set from this signal by `LongList::rebuild`, so the
        // row window (laid out from `self.offset`) and this paint always agree
        // on one offset — no one-frame lag that a fast fling could slide the
        // whole window off-screen through. A rebuild seeing the advanced offset
        // also reports `ChangeFlags::LAYOUT`, forcing the relayout Android's
        // frame gate would otherwise skip (see the module doc).
        let next = scripted_offset(self.script_ms, self.max_offset());
        if next != self.last_offset_published {
            self.last_offset_published = next;
            self.offset_signal.set(next);
        }
    }

    /// Publish (only on change) whether the scrolled viewport is nearing the
    /// not-yet-loaded edge — the paint-time signal channel the module doc's
    /// Pagination section describes.
    fn update_near_end(&mut self) {
        let loaded_extent = self.loaded as f64 * ITEM_EXTENT;
        let viewport_end = self.offset + self.viewport.height;
        let near = self.loaded < TOTAL_ROWS && viewport_end + NEAR_END_MARGIN_PX >= loaded_extent;
        if near != self.last_near_end_reported {
            self.last_near_end_reported = near;
            self.near_end.set(near);
        }
    }

    fn paint_row(&mut self, index: usize, row_origin: Point, scene: &mut dyn PaintScene) {
        let hue = ((index * 137) % 360) as f64;
        let (r, g, b) = hsv_to_rgb(hue, 0.55, 0.85);
        let thumb_size = Size::new(THUMB_SIZE, THUMB_SIZE);
        let thumb_origin = Point::new(
            row_origin.x + ROW_PAD,
            row_origin.y + (ITEM_EXTENT - THUMB_SIZE) / 2.0,
        );
        scene.fill_rect(thumb_origin, thumb_size, Color::from_rgb8(r, g, b));

        let text_x = thumb_origin.x + THUMB_SIZE + ROW_PAD;
        if index >= self.loaded {
            if let Some(label) = &mut self.loading_label {
                let mut pctx = PaintCtx::new(
                    Point::new(text_x, row_origin.y + (ITEM_EXTENT - 18.0) / 2.0),
                    Size::new(320.0, 18.0),
                );
                label.paint(&mut pctx, scene);
            }
            return;
        }
        if let Some(visual) = self.cache.get_mut(&index) {
            let mut title_ctx = PaintCtx::new(
                Point::new(text_x, row_origin.y + 10.0),
                Size::new(340.0, 18.0),
            );
            visual.title.paint(&mut title_ctx, scene);
            let mut subtitle_ctx = PaintCtx::new(
                Point::new(text_x, row_origin.y + 34.0),
                Size::new(340.0, 16.0),
            );
            visual.subtitle.paint(&mut subtitle_ctx, scene);
        }

        let icon_index = index % self.icon_widgets.len();
        let icon_widget = &mut self.icon_widgets[icon_index];
        let icon_origin = Point::new(
            row_origin.x + self.viewport.width - ICON_SIZE - ROW_PAD,
            row_origin.y + (ITEM_EXTENT - ICON_SIZE) / 2.0,
        );
        let mut icon_ctx = PaintCtx::new(icon_origin, Size::new(ICON_SIZE, ICON_SIZE));
        icon_widget.paint(&mut icon_ctx, scene);
    }
}

impl<State: 'static> View<State> for LongList {
    type Element = LongListWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LongListWidget {
        LongListWidget::new(self.loaded, self.near_end, self.offset)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LongListWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.near_end = self.near_end;
        element.offset_signal = self.offset;
        // The drawn/windowed offset is the signal value the previous paint
        // published — layout and paint both read `element.offset`, so they never
        // disagree (see `advance_script`).
        element.offset = self.scroll_offset;
        let mut flags = ChangeFlags::NONE;
        if prev.loaded != self.loaded {
            element.loaded = self.loaded;
            flags |= ChangeFlags::PAINT;
        }
        if prev.scroll_offset != self.scroll_offset {
            // The scripted scroll advanced (published from the previous paint):
            // force a relayout so the materialized row window re-windows around
            // the new offset — and new rows get shaped — even under Android's
            // otherwise layout-skipping frame gate (mirrors S6's `WidthBox`).
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for LongListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vw = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let vh = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            600.0
        };
        self.viewport = Size::new(vw, vh);
        self.offset = self.offset.clamp(0.0, self.max_offset());

        let (start, end) = self.desired_window();
        self.window = (start, end);

        let lo = start.saturating_sub(CACHE_MARGIN_ROWS);
        let hi = end + CACHE_MARGIN_ROWS;
        self.cache.retain(|&i, _| i >= lo && i < hi);
        for index in start..end {
            self.cache
                .entry(index)
                .or_insert_with(|| build_row_visual(ctx, index));
        }

        if self.loading_label.is_none() {
            let mut label = build_leaf(text("Loading…").size(13.0).color(LOADING_COLOR));
            label.layout(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, ITEM_EXTENT)),
            );
            self.loading_label = Some(label);
        }

        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        scene.fill_rect(origin, ctx.size(), BACKGROUND);
        scene.push_clip(origin, ctx.size());

        self.advance_script(ctx.frame_time());
        self.update_near_end();

        let (start, end) = self.window;
        for index in start..end {
            let y = index as f64 * ITEM_EXTENT - self.offset;
            if y + ITEM_EXTENT < 0.0 || y > self.viewport.height {
                continue;
            }
            let row_origin = Point::new(origin.x, origin.y + y);
            self.paint_row(index, row_origin, scene);
        }
        scene.pop_clip();

        // Perpetual scripted motion: always ask for the next frame.
        ctx.request_frame();
    }
}
