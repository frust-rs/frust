//! Ports beUI's `todo-list` agent-interface part.
//!
//! **Source:** `components/agents/todo-list.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `todo-list`: *"A collapsible agent task plan with morphing status
//! marks, a completion count, compact metadata, and smooth list updates."*
//!
//! The plan an agent publishes while it works: a header carrying a morphing
//! done/not-done glyph, a `completed / total` count whose leading number rolls
//! when it changes, a disclosure chevron, and a list of task rows whose status
//! mark morphs between four states — a dashed ring while pending, a progress
//! ring while running, a drawn check when complete, a drawn cross when
//! cancelled — with the completed row's title struck through.
//!
//! # Upstream's exports, and where each landed
//!
//! | upstream | here |
//! |---|---|
//! | `TodoList` | [`todo_list`] |
//! | `TodoItem` / `TodoItemStatus` | [`TodoListItem`] / [`TodoListStatus`] |
//! | `TodoHeaderIcon` | the header glyph, crossfaded on [`SPRING_SWAP`] |
//! | `TodoStatusIcon` | the per-row mark, driven by one clock per row |
//! | `ActionSwapRollText` on the completed count | the rolling count — see [`TODO_ROLL_MS`] |
//! | `AgentDisclosure` | the open/close reveal — see [`TODO_OPEN_MS`]/[`TODO_CLOSE_MS`] |
//! | `statusLabel` | [`TodoListStatus::label`] |
//!
//! # Geometry
//!
//! Every metric is upstream's own Tailwind class, resolved through
//! [`crate::style`]: the header is `h-11` ([`style::HEIGHT_INPUT`]) with
//! `px-3.5` and `gap-2.5`, a row is `min-h-9` with `px-1.5 py-1` and the same
//! `gap-2.5`, the list is `px-2 pb-2` inside a `rounded-2xl border` section, and
//! the viewport caps at `maxHeight = 248` ([`TODO_MAX_HEIGHT`]).
//!
//! # Motion
//!
//! Three clocks, none of them shared:
//!
//! * **The disclosure**, one lane retargeted between `0` and `1` on
//!   [`TODO_OPEN_MS`] opening and [`TODO_CLOSE_MS`] closing (upstream's
//!   `0.22`/`0.14`), driving the reveal's height, its clip, its opacity and its
//!   `y: -4` lift together. Because it drives **height**, `paint` asks for
//!   relayout rather than a bare frame while it runs — the same rule
//!   [`crate::components::animated_sidebar`]'s width morph states.
//! * **One clock per row**, latched when that row's status changes, from which
//!   every mark animation is read: the check draw ([`TODO_CHECK_MS`]), the
//!   cross draw ([`TODO_CROSS_MS`]), the progress ring
//!   (`SPRING_LAYOUT`) and the
//!   strike-through, which is the one animation upstream delays
//!   ([`TODO_STRIKE_DELAY_MS`] before [`TODO_STRIKE_MS`]). The delay is honoured
//!   exactly rather than folded into the ramp, because a row clock is a plain
//!   elapsed time this module owns rather than a lane.
//! * **One [`Presence`] per row**, so an appended task fades and settles in and
//!   a removed one plays its exit before its slot closes.
//!
//! # Degradations against the web original
//!
//! - **A removed row collapses its slot; upstream pops it out of flow.**
//!   `mode="popLayout"` takes an exiting row out of the layout immediately and
//!   fades it over the rows below. There is no out-of-flow layer in a frust
//!   layout pass, so the exiting row's own height shrinks with its presence
//!   instead. The rows below still slide up over the same interval.
//! - **The viewport follows the tail; it does not scroll.** Upstream's capped
//!   box is `overflow-y-auto` and is scrolled to the bottom whenever the item
//!   count changes. This port keeps the follow (the newest task is always
//!   visible) but publishes no scrollable viewport, so a reader cannot walk back
//!   up a long plan. A host that needs that nests the list in its own scroller.
//! - **The automatic collapse reports nothing.** Upstream's
//!   `collapseOnComplete` effect calls `onOpenChange` when the plan finishes. A
//!   callback here fires from event handling only, and completion arrives on a
//!   rebuild, so the automatic collapse changes the internal flag silently. A
//!   host that must know passes [`TodoListView::open`] and owns the flag itself.
//! - **Titles and details are strings, not arbitrary content.** Upstream takes a
//!   `ReactNode` for both. A row here shapes its own runs, which is what lets
//!   the strike-through be drawn across the measured title rather than over an
//!   arbitrary subtree.
//! - **No per-row focus and no keyboard.** The whole list is one widget, so
//!   there is no per-row focus target to ring and no `role="button"` header to
//!   activate from the keyboard — the same reasoning
//!   [`crate::components::animated_sidebar`] records for its rail.
//! - **The indeterminate progress ring does not spin.** Upstream rotates the
//!   ring forever when an `in-progress` item carries no `progress`. Here it
//!   rests at upstream's own default fraction ([`TODO_INDETERMINATE`]) instead,
//!   so the list requests no perpetual cosmetic frames for a plan that is
//!   otherwise still.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    Role, SemanticsCtx, Shape, Size, View, Widget, erase_callback_arg,
};
use frust::{FrameTime, Theme};
use kurbo::{Arc, Vec2};

use crate::motion::{Presence, Ramp};
use crate::press::{Lane, inside, presses};
use crate::style::{self, scale_alpha};
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT, SPRING_SWAP};
use crate::tokens::{BEUI_LIGHT, BeuiPalette, sans_family};

/// The header's height, in logical px (`h-11`).
pub const TODO_HEADER_HEIGHT: f64 = style::HEIGHT_INPUT;

/// Horizontal padding inside the header (`px-3.5`).
pub const TODO_HEADER_PADDING_X: f64 = 14.0;

/// The gap between the header's parts (`gap-2.5`), shared with a row.
pub const TODO_GAP: f64 = 10.0;

/// The header glyph's box (`size-6`).
pub const TODO_HEADER_ICON: f64 = 24.0;

/// A task row's smallest height (`min-h-9`).
pub const TODO_ROW_MIN_HEIGHT: f64 = 36.0;

/// Horizontal padding inside a task row (`px-1.5`).
pub const TODO_ROW_PADDING_X: f64 = 6.0;

/// Vertical padding inside a task row (`py-1`).
pub const TODO_ROW_PADDING_Y: f64 = 4.0;

/// Horizontal padding of the list viewport (`px-2`), and its bottom padding
/// (`pb-2`).
pub const TODO_LIST_PADDING: f64 = 8.0;

/// A row's status mark box (`size-5`), with `mx-0.5` on either side.
pub const TODO_MARK_SIZE: f64 = 20.0;

/// The default height the list viewport caps at (`maxHeight = 248`).
pub const TODO_MAX_HEIGHT: f64 = 248.0;

/// How long the disclosure takes to open, in ms (`open ? 0.22`).
pub const TODO_OPEN_MS: u64 = 220;

/// How long the disclosure takes to close, in ms (`: 0.14`).
pub const TODO_CLOSE_MS: u64 = 140;

/// How far the closed disclosure sits above its open position, in logical px
/// (`y: open ? 0 : -4`).
pub const TODO_DISCLOSURE_LIFT: f64 = 4.0;

/// How long a row's entrance fade takes, in ms (`opacity: { duration: 0.18 }`).
pub const TODO_ROW_FADE_MS: u64 = 180;

/// How far a row rises through its entrance, in logical px (`y: 6`).
pub const TODO_ROW_ENTER_SHIFT: f64 = 6.0;

/// How far a row lifts through its exit, in logical px (`exit: { y: -3 }`).
pub const TODO_ROW_EXIT_SHIFT: f64 = 3.0;

/// How long the completed row's strike-through takes to draw, in ms
/// (`duration: 0.28`).
pub const TODO_STRIKE_MS: u64 = 280;

/// How long the strike-through waits before drawing, in ms (`delay: 0.06`).
pub const TODO_STRIKE_DELAY_MS: u64 = 60;

/// How long the completed mark's check draws for, in ms (`duration: 0.24`).
pub const TODO_CHECK_MS: u64 = 240;

/// How long the cancelled mark's cross draws for, in ms (`duration: 0.2`).
pub const TODO_CROSS_MS: u64 = 200;

/// How long the mark's fill fades over, in ms (`duration: 0.18`).
pub const TODO_MARK_FILL_MS: u64 = 180;

/// How long the completed count's roll takes, in ms — the roll upstream gets
/// from `ActionSwapRollText`, timed here on the catalog's own swap spring.
pub const TODO_ROLL_MS: u64 = 260;

/// The fraction an `in-progress` ring shows when its item carries no explicit
/// progress (upstream's `normalizedProgress` default of `0.68`).
pub const TODO_INDETERMINATE: f64 = 0.68;

/// Alpha of a completed mark's fill (`fillOpacity: 0.06`).
const MARK_FILL_ALPHA: f32 = 0.06;

/// Alpha of the ring behind an `in-progress` mark (`opacity-20`).
const MARK_TRACK_ALPHA: f32 = 0.2;

/// Alpha of a pending row's title (`text-muted-foreground/65`).
const PENDING_TITLE_ALPHA: f32 = 0.65;

/// Alpha of a completed row's title (`text-muted-foreground/60`).
const COMPLETED_TITLE_ALPHA: f32 = 0.6;

/// Alpha of a cancelled row's title (`text-muted-foreground/55`), shared with a
/// row's trailing detail (`text-muted-foreground/55`).
const DIM_TITLE_ALPHA: f32 = 0.55;

/// Alpha of the section's hairline (`border-border/70`).
const SECTION_BORDER_ALPHA: f32 = 0.7;

/// Alpha of the header's title ink (`text-foreground/90`).
const HEADER_TITLE_ALPHA: f32 = 0.9;

/// The mark's stroke width, in logical px, scaled from upstream's `24`-unit
/// viewBox (`strokeWidth="2"` at [`TODO_MARK_SIZE`]).
const MARK_STROKE: f64 = 2.0 * TODO_MARK_SIZE / 24.0;

/// The mark ring's radius, from the same viewBox (`r="9"`).
const MARK_RADIUS: f64 = 9.0 * TODO_MARK_SIZE / 24.0;

/// The default header title (`title = "To-dos"`).
pub const TODO_TITLE: &str = "To-dos";

/// What the list says when it holds no tasks (`"No tasks yet"`).
pub const TODO_EMPTY_LABEL: &str = "No tasks yet";

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// Where one task stands — upstream's `TodoItemStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TodoListStatus {
    /// Queued: a dashed ring, dimmed title.
    #[default]
    Pending,
    /// Running: a progress ring, full-strength title.
    InProgress,
    /// Done: a drawn check, a filled ring and a struck-through title.
    Completed,
    /// Abandoned: a drawn cross in the destructive role.
    Cancelled,
}

impl TodoListStatus {
    /// Every status, in upstream's own union order.
    pub const ALL: [TodoListStatus; 4] = [
        TodoListStatus::Pending,
        TodoListStatus::InProgress,
        TodoListStatus::Completed,
        TodoListStatus::Cancelled,
    ];

    /// The accessible label upstream's `statusLabel` gives this status.
    pub const fn label(self) -> &'static str {
        match self {
            TodoListStatus::Pending => "Pending",
            TodoListStatus::InProgress => "In progress",
            TodoListStatus::Completed => "Completed",
            TodoListStatus::Cancelled => "Cancelled",
        }
    }

    /// Whether this status counts toward the header's completion tally.
    pub const fn is_complete(self) -> bool {
        matches!(self, TodoListStatus::Completed)
    }
}

/// One task in the plan — upstream's `TodoItem`.
///
/// `id` is the identity a rebuild diffs against: a row keeps its retained
/// motion state as long as its id survives, and an id that appears or
/// disappears is what stages an entrance or an exit.
#[derive(Clone, Debug, PartialEq)]
pub struct TodoListItem {
    id: String,
    title: String,
    status: TodoListStatus,
    progress: Option<f64>,
    detail: Option<String>,
}

/// Create a [`Pending`](TodoListStatus::Pending) task identified by `id`.
pub fn todo_item(id: impl Into<String>, title: impl Into<String>) -> TodoListItem {
    TodoListItem {
        id: id.into(),
        title: title.into(),
        status: TodoListStatus::default(),
        progress: None,
        detail: None,
    }
}

impl TodoListItem {
    /// Give the task `status` instead of [`Pending`](TodoListStatus::Pending).
    pub fn status(mut self, status: TodoListStatus) -> Self {
        self.status = status;
        self
    }

    /// Give an [`InProgress`](TodoListStatus::InProgress) task a definite
    /// completion percentage (`0..=100`, upstream's own range).
    pub fn progress(mut self, percent: f64) -> Self {
        self.progress = Some(percent.clamp(0.0, 100.0));
        self
    }

    /// Add the trailing metadata upstream's `detail` slot carries.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// This task's identity.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// This task's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The ring fraction this task's mark shows: its own progress, or
    /// [`TODO_INDETERMINATE`] when it declares none.
    pub fn ring_fraction(&self) -> f64 {
        self.progress.map_or(TODO_INDETERMINATE, |p| p / 100.0)
    }
}

/// A view-held, typed disclosure callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI agent task list. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::agents::todo_list::{TodoListStatus, todo_item, todo_list};
///
/// let plan = todo_list::<()>(vec![
///     todo_item("audit", "Audit the checkout flow").status(TodoListStatus::Completed),
///     todo_item("fix", "Close the validation gap").status(TodoListStatus::InProgress),
///     todo_item("ship", "Prepare the patch"),
/// ])
/// .title("Release plan");
/// ```
pub struct TodoListView<State: 'static> {
    items: Vec<TodoListItem>,
    title: String,
    open: Option<bool>,
    default_open: bool,
    collapse_on_complete: bool,
    max_height: f64,
    on_open_change: Option<OnOpenChange<State>>,
}

/// Create an open task list over `items`, titled [`TODO_TITLE`].
pub fn todo_list<State: 'static>(items: Vec<TodoListItem>) -> TodoListView<State> {
    TodoListView {
        items,
        title: TODO_TITLE.to_owned(),
        open: None,
        default_open: true,
        collapse_on_complete: true,
        max_height: TODO_MAX_HEIGHT,
        on_open_change: None,
    }
}

impl<State: 'static> TodoListView<State> {
    /// Replace the header title (default [`TODO_TITLE`]).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Take ownership of the disclosure: the list then shows exactly `open` and
    /// never flips it itself, neither on a press nor on completion.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// The disclosure state an *uncontrolled* list starts at (`defaultOpen`,
    /// upstream's own default `true`).
    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Whether an uncontrolled list collapses itself when every task completes,
    /// and re-opens when one stops being complete (`collapseOnComplete`).
    pub fn collapse_on_complete(mut self, collapse: bool) -> Self {
        self.collapse_on_complete = collapse;
        self
    }

    /// Cap the task viewport at `height` logical px (default
    /// [`TODO_MAX_HEIGHT`]).
    pub fn max_height(mut self, height: f64) -> Self {
        self.max_height = height.max(0.0);
        self
    }

    /// Observe presses on the header: reports the state the list is being asked
    /// to move to.
    ///
    /// Fired from the event pass only, so the automatic completion collapse —
    /// which happens on a rebuild — reports nothing; see the [module
    /// docs](self)' *Degradations*.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, callback: F) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// How many of this view's tasks are complete.
    pub fn completed(&self) -> usize {
        self.items.iter().filter(|i| i.status.is_complete()).count()
    }
}

/// The resolved list palette.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TodoColors {
    /// The section's hairline (`border-border/70`).
    border: Color,
    /// Full-strength ink (`text-foreground`).
    ink: Color,
    /// Secondary ink (`text-muted-foreground`).
    muted: Color,
    /// The completed mark's accent (`text-emerald-500`, resolved to the
    /// catalog's success role).
    success: Color,
    /// The cancelled mark's accent (`text-rose-600`).
    danger: Color,
}

impl TodoColors {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                TodoColors {
                    border: scheme.outline_variant,
                    ink: scheme.on_surface,
                    muted: scheme.on_surface_variant,
                    success: scheme.tertiary,
                    danger: scheme.error,
                }
            }
            None => TodoColors {
                border: FALLBACK.border,
                ink: FALLBACK.foreground,
                muted: FALLBACK.muted_foreground,
                success: FALLBACK.success,
                danger: FALLBACK.danger,
            },
        }
    }
}

/// A row's own text style: `text-sm leading-5`, regular weight.
fn row_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        ..TextStyle::default()
    }
}

/// The header's title style: `text-sm font-medium`.
fn header_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        weight: FontWeight::MEDIUM,
        ..TextStyle::default()
    }
}

/// The count's style: `text-xs font-medium tabular-nums`.
fn count_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_XS as f32,
        weight: FontWeight::MEDIUM,
        ..TextStyle::default()
    }
}

/// One retained task row.
struct TodoRow {
    /// The identity a rebuild matches on.
    id: String,
    /// The row's title, re-brushed per paint (its ink follows its status).
    title: LabelRun,
    /// The trailing metadata, when the task carries any.
    detail: Option<LabelRun>,
    /// The status the marks are currently animating toward.
    status: TodoListStatus,
    /// The status the marks are animating *from*, so a check can fade out as a
    /// cross draws in.
    previous: TodoListStatus,
    /// The ring fraction an `in-progress` mark shows.
    fraction: f64,
    /// The frame the current status change was first painted at; `None` until
    /// the first paint that carries a clock.
    marked_at: Option<FrameTime>,
    /// Whether a mark animation is still running. A row that mounts with its
    /// status already settled never plays one, which is what keeps a page of
    /// history from drawing a page of checks.
    marks_running: bool,
    /// This row's enter/exit driver.
    presence: Presence,
    /// The row's resolved height, as of the last layout.
    height: f64,
    /// The row's title width, as of the last layout — what the strike-through
    /// is drawn across.
    title_width: f64,
    /// How present the row was on the last paint. Written by `paint`, read by
    /// `layout`, which is what lets an exiting row's slot close over its exit
    /// ramp rather than snapping shut.
    shown: f64,
}

impl TodoRow {
    /// A fresh row for `item`, closed (its entrance is opened by the rebuild or
    /// the first layout that mounts it).
    fn new(item: &TodoListItem) -> Self {
        TodoRow {
            id: item.id.clone(),
            title: LabelRun::new(item.title.clone()),
            detail: item.detail.clone().map(LabelRun::new),
            status: item.status,
            previous: item.status,
            fraction: item.ring_fraction(),
            marked_at: None,
            marks_running: false,
            presence: Presence::new(
                Ramp::spring(SPRING_LAYOUT),
                Ramp::eased(Duration::from_millis(TODO_ROW_FADE_MS), EASE_OUT),
            ),
            height: TODO_ROW_MIN_HEIGHT,
            title_width: 0.0,
            shown: 0.0,
        }
    }

    /// Adopt `item`'s content, reporting what changed. A status change latches
    /// a fresh mark clock so every mark animation restarts together.
    fn adopt(&mut self, item: &TodoListItem) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.title.set_content(item.title.clone()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let detail_changed = match (&mut self.detail, &item.detail) {
            (Some(run), Some(text)) => run.set_content(text.clone()),
            (slot @ Some(_), None) => {
                *slot = None;
                true
            }
            (slot @ None, Some(text)) => {
                *slot = Some(LabelRun::new(text.clone()));
                true
            }
            (None, None) => false,
        };
        if detail_changed {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if self.status != item.status {
            self.previous = self.status;
            self.status = item.status;
            self.marked_at = None;
            self.marks_running = true;
            flags |= ChangeFlags::PAINT;
        }
        if self.fraction != item.ring_fraction() {
            self.fraction = item.ring_fraction();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    /// How long this row's marks have been running at `now`. A row with no
    /// animation in flight reports [`MARKS_AT_REST`], so every mark reads as
    /// fully drawn without a clock of its own.
    fn mark_elapsed(&mut self, now: FrameTime) -> Duration {
        if !self.marks_running {
            return MARKS_AT_REST;
        }
        let started = *self.marked_at.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if marks_settled(elapsed) {
            self.marks_running = false;
            return MARKS_AT_REST;
        }
        elapsed
    }
}

/// The elapsed time a settled row's marks are read at — past every mark ramp,
/// so each reads as finished.
const MARKS_AT_REST: Duration = Duration::from_secs(60);

/// Whether every mark animation has finished by `elapsed`.
fn marks_settled(elapsed: Duration) -> bool {
    elapsed
        >= Duration::from_millis(TODO_STRIKE_DELAY_MS + TODO_STRIKE_MS)
            .max(Ramp::spring(SPRING_LAYOUT).settle())
}

/// The retained widget for a [`TodoListView`].
pub struct TodoListWidget {
    rows: Vec<TodoRow>,
    header: LabelRun,
    /// The `completed` half of the count, and the value it rolled from.
    count: LabelRun,
    previous_count: LabelRun,
    /// The `/total` half, which never rolls.
    total: LabelRun,
    /// The empty-list message, shaped only while the list is empty.
    empty: LabelRun,
    /// The count roll, `0` settled on the previous value .. `1` on the current.
    roll: Lane,
    /// The disclosure, `0` closed .. `1` open.
    reveal: Lane,
    /// The header glyph morph, `0` not-done .. `1` done.
    header_mark: Lane,
    /// The viewport's follow offset, `<= 0` while the content overflows.
    follow: Lane,
    /// The app-owned flag, when the list is controlled.
    controlled: Option<bool>,
    /// The list's own flag, when it is not.
    internal_open: bool,
    /// Whether every task was complete as of the last rebuild — the edge the
    /// automatic collapse triggers on.
    was_complete: bool,
    collapse_on_complete: bool,
    max_height: f64,
    /// Whether the header is currently held down.
    pressed: bool,
    /// The header's box, resolved by layout.
    header_box: Rect,
    /// The viewport's box, resolved by layout.
    viewport: Rect,
    /// The rows' total height, as of the last layout.
    content_height: f64,
    on_open_change: Option<ErasedArgCallback<bool>>,
}

impl TodoListWidget {
    /// Whether the disclosure is open right now.
    pub fn is_open(&self) -> bool {
        self.controlled.unwrap_or(self.internal_open)
    }

    /// How far the disclosure has opened, `0` closed .. `1` open.
    pub fn reveal_progress(&self) -> f64 {
        self.reveal.value()
    }

    /// How many rows the list holds, including any still playing an exit.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// How many of its rows are complete.
    pub fn completed(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.presence.is_visible() && row.status.is_complete())
            .count()
    }

    /// The header's box, as of the last layout.
    pub fn header_box(&self) -> Rect {
        self.header_box
    }

    /// The task viewport's box, as of the last layout.
    pub fn viewport_box(&self) -> Rect {
        self.viewport
    }

    /// The rows' unclipped height, as of the last layout.
    pub fn content_height(&self) -> f64 {
        self.content_height
    }

    /// How far row `index` has been struck through, `0` untouched .. `1` fully
    /// drawn. A row that is not complete reports `0`.
    pub fn strike_progress(&self, index: usize, now: FrameTime) -> f64 {
        let Some(row) = self.rows.get(index) else {
            return 0.0;
        };
        if !row.status.is_complete() {
            return 0.0;
        }
        match row.marked_at.filter(|_| row.marks_running) {
            Some(started) => strike_at(now.saturating_sub(started)),
            None => strike_at(MARKS_AT_REST),
        }
    }

    /// How present row `index` is, `0` gone .. `1` settled.
    pub fn row_presence(&self, index: usize, now: FrameTime) -> f64 {
        self.rows
            .get(index)
            .map_or(0.0, |row| row.presence.presence(now))
    }

    /// Retarget the disclosure onto the ramp its new direction is timed by.
    fn aim_reveal(&mut self, open: bool, reduce_motion: bool) {
        let ramp = Ramp::eased(
            Duration::from_millis(if open { TODO_OPEN_MS } else { TODO_CLOSE_MS }),
            EASE_OUT,
        );
        self.reveal
            .retarget_with(ramp, if open { 1.0 } else { 0.0 });
        if reduce_motion {
            self.reveal.snap();
        }
    }
}

/// The strike-through's drawn fraction at `elapsed` past the status change:
/// nothing for [`TODO_STRIKE_DELAY_MS`], then [`TODO_STRIKE_MS`] of
/// [`EASE_OUT`].
fn strike_at(elapsed: Duration) -> f64 {
    let delay = Duration::from_millis(TODO_STRIKE_DELAY_MS);
    if elapsed <= delay {
        return 0.0;
    }
    Ramp::eased(Duration::from_millis(TODO_STRIKE_MS), EASE_OUT).progress_clamped(elapsed - delay)
}

/// A polyline drawn to `fraction` of its own length — Motion's `pathLength`,
/// which is how both the check and the cross are drawn.
fn drawn_polyline(points: &[(f64, f64)], fraction: f64) -> BezPath {
    let mut path = BezPath::new();
    if points.len() < 2 || fraction <= 0.0 {
        return path;
    }
    let lengths: Vec<f64> = points
        .windows(2)
        .map(|pair| {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt()
        })
        .collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return path;
    }
    let mut remaining = total * fraction.min(1.0);
    path.move_to(Point::new(points[0].0, points[0].1));
    for (index, length) in lengths.iter().enumerate() {
        let (x0, y0) = points[index];
        let (x1, y1) = points[index + 1];
        if remaining >= *length {
            path.line_to(Point::new(x1, y1));
            remaining -= *length;
            continue;
        }
        let t = if *length > 0.0 {
            remaining / length
        } else {
            0.0
        };
        path.line_to(Point::new(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
        break;
    }
    path
}

/// Upstream's check path, in its own `24`-unit viewBox
/// (`M7.5 12.25 10.5 15.25 16.75 8.75`).
const CHECK_POINTS: [(f64, f64); 3] = [(7.5, 12.25), (10.5, 15.25), (16.75, 8.75)];

/// Upstream's cross, whose two strokes are drawn as two polylines
/// (`M8.5 8.5 15.5 15.5M15.5 8.5 8.5 15.5`).
const CROSS_POINTS: [[(f64, f64); 2]; 2] = [[(8.5, 8.5), (15.5, 15.5)], [(15.5, 8.5), (8.5, 15.5)]];

/// Scale a viewBox point onto a `TODO_MARK_SIZE` box at `origin`.
fn mark_point(origin: Point, point: (f64, f64)) -> (f64, f64) {
    let scale = TODO_MARK_SIZE / 24.0;
    (origin.x + point.0 * scale, origin.y + point.1 * scale)
}

impl<State: 'static> View<State> for TodoListView<State> {
    type Element = TodoListWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TodoListWidget {
        let completed = self.completed();
        let all_complete = !self.items.is_empty() && completed == self.items.len();
        let open = self
            .open
            .unwrap_or(!(all_complete && self.collapse_on_complete) && self.default_open);
        let mut rows: Vec<TodoRow> = self.items.iter().map(TodoRow::new).collect();
        // A list mounts settled: a page of history must not play a page of
        // entrances, exactly as `message_bubble`'s `animateIn` default states.
        for row in &mut rows {
            row.presence.set_open(true);
            row.presence.advance(FrameTime::from_nanos(0));
            row.presence.advance(FrameTime::from_nanos(u64::MAX / 2));
            row.shown = 1.0;
        }
        TodoListWidget {
            rows,
            header: LabelRun::new(self.title.clone()),
            count: LabelRun::new(completed.to_string()),
            previous_count: LabelRun::new(completed.to_string()),
            total: LabelRun::new(format!("/{}", self.items.len())),
            empty: LabelRun::new(TODO_EMPTY_LABEL),
            roll: Lane::at_rest(Ramp::spring(SPRING_SWAP), 1.0),
            reveal: Lane::at_rest(
                Ramp::eased(Duration::from_millis(TODO_OPEN_MS), EASE_OUT),
                if open { 1.0 } else { 0.0 },
            ),
            header_mark: Lane::at_rest(
                Ramp::spring(SPRING_SWAP),
                if all_complete { 1.0 } else { 0.0 },
            ),
            follow: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0),
            controlled: self.open,
            internal_open: open,
            was_complete: all_complete,
            collapse_on_complete: self.collapse_on_complete,
            max_height: self.max_height,
            pressed: false,
            header_box: Rect::ZERO,
            viewport: Rect::ZERO,
            content_height: 0.0,
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TodoListWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if element.header.set_content(self.title.clone()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Reconcile the rows by id: a surviving row keeps its motion state, a
        // new id enters, and an id that has gone stays mounted for its exit.
        if prev.items != self.items {
            let mut kept: Vec<TodoRow> = Vec::with_capacity(self.items.len());
            let mut previous = std::mem::take(&mut element.rows);
            for item in &self.items {
                match previous.iter().position(|row| row.id == item.id) {
                    Some(index) => {
                        let mut row = previous.remove(index);
                        flags |= row.adopt(item);
                        // An id that comes back mid-exit re-enters rather than
                        // finishing its departure.
                        if row.presence.set_open(true) {
                            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                        }
                        kept.push(row);
                    }
                    None => {
                        let mut row = TodoRow::new(item);
                        row.presence.set_open(true);
                        kept.push(row);
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
            }
            // Whatever the view no longer names leaves, keeping its slot until
            // the exit ramp finishes.
            for mut gone in previous {
                if gone.presence.is_visible() {
                    gone.presence.set_open(false);
                    kept.push(gone);
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
            element.rows = kept;
        }

        let completed = self.completed();
        let all_complete = !self.items.is_empty() && completed == self.items.len();
        if element.count.content() != completed.to_string() {
            element
                .previous_count
                .set_content(element.count.content().to_owned());
            element.count.set_content(completed.to_string());
            element.roll = Lane::at_rest(Ramp::spring(SPRING_SWAP), 0.0);
            element.roll.retarget(1.0);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.total.set_content(format!("/{}", self.items.len())) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        element.collapse_on_complete = self.collapse_on_complete;
        element.controlled = self.open;
        if element.max_height != self.max_height {
            element.max_height = self.max_height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Upstream's completion effect: an uncontrolled list folds itself away
        // when the plan finishes and re-opens when it stops being finished.
        if element.was_complete != all_complete {
            if element.controlled.is_none() {
                if all_complete && self.collapse_on_complete {
                    element.internal_open = false;
                } else if !all_complete {
                    element.internal_open = true;
                }
            }
            element.was_complete = all_complete;
            element
                .header_mark
                .retarget(if all_complete { 1.0 } else { 0.0 });
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.open != self.open {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        flags
    }
}

impl Widget for TodoListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Drop the rows whose exit finished on the last paint; their slot is
        // what closes here.
        self.rows.retain(|row| row.presence.is_visible());

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        self.header_box =
            Rect::from_origin_size(Point::ORIGIN, Size::new(width, TODO_HEADER_HEIGHT));

        let header_style = header_style();
        let count_style = count_style();
        self.header.layout(ctx, &header_style);
        self.count.layout(ctx, &count_style);
        self.previous_count.layout(ctx, &count_style);
        self.total.layout(ctx, &count_style);

        let row_style = row_style();
        let inner_width = (width - TODO_LIST_PADDING * 2.0 - TODO_ROW_PADDING_X * 2.0).max(0.0);
        let mut content = 0.0;
        for row in &mut self.rows {
            let detail_width = match &mut row.detail {
                Some(detail) => detail.layout(ctx, &row_style).width + TODO_GAP,
                None => 0.0,
            };
            let title_max = (inner_width - TODO_MARK_SIZE - TODO_GAP - detail_width).max(0.0);
            let title = row.title.layout(ctx, &row_style);
            // `truncate`: the run is shaped on one line and clipped to the room
            // left over, never wrapped — the framework's shaper takes no wrap
            // width through the catalog's cached-run seam.
            row.title_width = title.width.min(title_max);
            let natural = (title.height + TODO_ROW_PADDING_Y * 2.0).max(TODO_ROW_MIN_HEIGHT);
            // An exiting row's slot closes with its presence, which is what the
            // module docs call out as the popLayout stand-in.
            row.height = natural;
            content += natural * row.shown.clamp(0.0, 1.0);
        }
        if self.rows.is_empty() {
            self.empty.layout(ctx, &row_style);
            content = self.empty.size().height + TODO_ROW_PADDING_Y * 2.0;
        }
        self.content_height = content;

        let capped = content.min(self.max_height);
        let open_height = capped + TODO_LIST_PADDING;
        let revealed = (open_height * self.reveal.value().clamp(0.0, 1.0)).max(0.0);
        self.viewport = Rect::from_origin_size(
            Point::new(TODO_LIST_PADDING, TODO_HEADER_HEIGHT),
            Size::new((width - TODO_LIST_PADDING * 2.0).max(0.0), capped),
        );
        bc.constrain(Size::new(width, TODO_HEADER_HEIGHT + revealed))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, colors) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                TodoColors::resolve(theme),
            )
        };
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();
        let open = self.is_open();
        self.aim_reveal(open, reduce_motion);

        let mut owes_layout = self.reveal.advance(now);
        if reduce_motion {
            self.roll.snap();
            self.header_mark.snap();
            self.follow.snap();
        }
        let mut owes_frame = self.roll.advance(now);
        owes_frame |= self.header_mark.advance(now);

        // The section: a `rounded-2xl` box with a `border-border/70` hairline.
        let radius = style::resolve_radius(style::RADIUS_2XL, size.width, size.height);
        crate::press::stroke_outline(
            scene,
            origin,
            size,
            radius,
            scale_alpha(colors.border, SECTION_BORDER_ALPHA),
        );

        self.paint_header(ctx, scene, &colors, origin, size, reduce_motion);

        let revealed = self.reveal.value().clamp(0.0, 1.0);
        if revealed > 0.0 {
            let lift = TODO_DISCLOSURE_LIFT * (1.0 - revealed);
            let clip_height = (size.height - TODO_HEADER_HEIGHT).max(0.0);
            let clip_origin = Point::new(origin.x, origin.y + TODO_HEADER_HEIGHT);
            scene.push_clip(clip_origin, Size::new(size.width, clip_height));
            scene.push_layer(
                clip_origin,
                Size::new(size.width, clip_height),
                revealed as f32,
            );
            let follow_target = (self.viewport.height() - self.content_height).min(0.0);
            if reduce_motion {
                self.follow.retarget(follow_target);
                self.follow.snap();
            } else {
                self.follow.retarget(follow_target);
                owes_frame |= self.follow.advance(now);
            }
            let list_origin = Point::new(
                origin.x + TODO_LIST_PADDING,
                origin.y + TODO_HEADER_HEIGHT - lift + self.follow.value(),
            );
            owes_frame |= self.paint_rows(ctx, scene, &colors, list_origin, reduce_motion, now);
            scene.pop_layer();
            scene.pop_clip();
        }

        // A height animation must drive layout, not just repaint — the mobile
        // intra-frame layout skip would otherwise freeze the reveal mid-flight.
        owes_layout |= self.reveal.value() != self.reveal.target();
        if owes_layout {
            ctx.request_layout();
        } else if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let hits_header = self.header_box.contains(p.position);
        match p.phase {
            PointerPhase::Down if hits_header && presses(p) => {
                self.pressed = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if hits_header && inside(p.position, ctx.size()) {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                EventResult::Ignored
            }
            PointerPhase::Up if self.pressed => {
                self.pressed = false;
                if hits_header {
                    let next = !self.is_open();
                    if self.controlled.is_none() {
                        self.internal_open = next;
                    }
                    if let Some(callback) = &mut self.on_open_change {
                        callback(ctx, next);
                    }
                    // `EventCtx` carries no relayout request; the reveal lane is
                    // now aimed somewhere new, so the next paint asks for one.
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Cancel if self.pressed => {
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let open = self.is_open();
        let completed = self.completed();
        let total = self.rows.iter().filter(|r| r.presence.is_visible()).count();
        ctx.push_container(
            Role::Group,
            |node| {
                node.set_label(format!(
                    "{}: {completed} of {total} tasks completed",
                    self.header.content()
                ));
                node.add_action(Action::Click);
                node.set_expanded(open);
            },
            |ctx| {
                if !open {
                    return;
                }
                for row in self.rows.iter().filter(|r| r.presence.is_visible()) {
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(format!("{}: {}", row.status.label(), row.title.content()));
                    });
                }
            },
        );
    }
}

impl TodoListWidget {
    /// Paint the header: morphing glyph, title, rolling count, chevron.
    fn paint_header(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        colors: &TodoColors,
        origin: Point,
        size: Size,
        reduce_motion: bool,
    ) {
        let done = self.header_mark.value().clamp(0.0, 1.0);
        let icon_origin = Point::new(
            origin.x + TODO_HEADER_PADDING_X,
            origin.y + (TODO_HEADER_HEIGHT - TODO_HEADER_ICON) / 2.0,
        );

        // The not-done glyph: three short rules, upstream's `ListTodo`.
        if done < 1.0 {
            let ink = scale_alpha(colors.muted, (1.0 - done) as f32);
            let left = icon_origin.x + 5.0;
            for step in 0..3 {
                let y = icon_origin.y + 7.0 + step as f64 * 5.0;
                scene.stroke_line(
                    Point::new(left + 6.0, y),
                    Point::new(left + 13.0, y),
                    1.6,
                    ink,
                );
                scene.stroke_line(Point::new(left, y), Point::new(left + 3.0, y), 1.6, ink);
            }
        }
        // The done glyph: a filled disc with a drawn check.
        if done > 0.0 {
            let centre = Point::new(
                icon_origin.x + TODO_HEADER_ICON / 2.0,
                icon_origin.y + TODO_HEADER_ICON / 2.0,
            );
            let radius = 9.0 * TODO_HEADER_ICON / 24.0 * done;
            let disc = Arc::new(
                centre,
                Vec2::new(radius, radius),
                0.0,
                std::f64::consts::TAU,
                0.0,
            );
            scene.fill_path(
                Point::ORIGIN,
                &disc.to_path(style::PATH_TOLERANCE),
                &Brush::Solid(scale_alpha(colors.success, done as f32)),
            );
            let mark_origin = Point::new(
                centre.x - TODO_HEADER_ICON / 2.0,
                centre.y - TODO_HEADER_ICON / 2.0,
            );
            let scale = TODO_HEADER_ICON / 24.0;
            let points: Vec<(f64, f64)> = CHECK_POINTS
                .iter()
                .map(|p| (mark_origin.x + p.0 * scale, mark_origin.y + p.1 * scale))
                .collect();
            scene.stroke_path(
                Point::ORIGIN,
                &drawn_polyline(&points, done),
                2.25 * scale,
                &Brush::Solid(Color::WHITE),
            );
        }

        let text_left = icon_origin.x + TODO_HEADER_ICON + TODO_GAP;
        let count_width = self
            .count
            .size()
            .width
            .max(self.previous_count.size().width)
            + self.total.size().width;
        let chevron_span = style::ICON_SIZE - 2.0 + TODO_GAP;
        let count_left = origin.x + size.width - TODO_HEADER_PADDING_X - chevron_span - count_width;

        self.header.paint(
            Point::new(
                text_left,
                origin.y + (TODO_HEADER_HEIGHT - self.header.size().height) / 2.0,
            ),
            scale_alpha(colors.ink, HEADER_TITLE_ALPHA),
            scene,
        );

        // The count: the completed half rolls up as it changes, the total does
        // not — upstream's `ActionSwapRollText` wraps only the first span.
        let roll = self.roll.value().clamp(0.0, 1.0);
        let count_ink = if self.header_mark.target() >= 1.0 {
            colors.success
        } else {
            colors.muted
        };
        let count_y = origin.y + (TODO_HEADER_HEIGHT - self.count.size().height) / 2.0;
        let travel = self.count.size().height;
        if roll < 1.0 {
            self.previous_count.paint(
                Point::new(count_left, count_y - travel * roll),
                scale_alpha(count_ink, (1.0 - roll) as f32),
                scene,
            );
        }
        self.count.paint(
            Point::new(count_left, count_y + travel * (1.0 - roll)),
            scale_alpha(count_ink, roll as f32),
            scene,
        );
        self.total.paint(
            Point::new(count_left + self.count.size().width, count_y),
            count_ink,
            scene,
        );

        // The chevron, rotated 180° while open.
        let turn = if reduce_motion {
            if self.is_open() { 1.0 } else { 0.0 }
        } else {
            self.reveal.value().clamp(0.0, 1.0)
        };
        let chevron_centre = Point::new(
            origin.x + size.width - TODO_HEADER_PADDING_X - (style::ICON_SIZE - 2.0) / 2.0,
            origin.y + TODO_HEADER_HEIGHT / 2.0,
        );
        let arm = (style::ICON_SIZE - 2.0) / 2.0 - 1.0;
        let drop = (1.0 - 2.0 * turn) * arm * 0.5;
        let ink = scale_alpha(colors.muted, 0.5 + 0.5 * turn as f32);
        scene.stroke_line(
            Point::new(chevron_centre.x - arm, chevron_centre.y - drop),
            Point::new(chevron_centre.x, chevron_centre.y + drop),
            1.5,
            ink,
        );
        scene.stroke_line(
            Point::new(chevron_centre.x, chevron_centre.y + drop),
            Point::new(chevron_centre.x + arm, chevron_centre.y - drop),
            1.5,
            ink,
        );
        let _ = ctx;
    }

    /// Paint the task rows, returning whether any of them still owes a frame.
    fn paint_rows(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        colors: &TodoColors,
        list_origin: Point,
        reduce_motion: bool,
        now: FrameTime,
    ) -> bool {
        if self.rows.is_empty() {
            self.empty.paint(
                Point::new(
                    list_origin.x + TODO_ROW_PADDING_X,
                    list_origin.y + TODO_ROW_PADDING_Y,
                ),
                colors.muted,
                scene,
            );
            return false;
        }

        let mut owes_frame = false;
        let mut y = list_origin.y;
        let width = self.viewport.width();
        for row in &mut self.rows {
            if reduce_motion {
                // Reduced motion draws every mark at rest rather than playing
                // a shorter version of the morph.
                row.marks_running = false;
            }
            let presence = row.presence.advance(now);
            owes_frame |= row.presence.is_animating();
            if presence <= 0.0 {
                continue;
            }
            let alpha = presence.clamp(0.0, 1.0);
            row.shown = alpha;
            let shift = match row.presence.phase() {
                crate::motion::PresencePhase::Exiting => -TODO_ROW_EXIT_SHIFT * (1.0 - alpha),
                _ => TODO_ROW_ENTER_SHIFT * (1.0 - alpha),
            };
            let row_origin = Point::new(list_origin.x + TODO_ROW_PADDING_X, y + shift);
            let elapsed = row.mark_elapsed(now);
            if row.marks_running {
                owes_frame = true;
            }

            if alpha < 1.0 {
                scene.push_layer(
                    Point::new(list_origin.x, y),
                    Size::new(width, row.height),
                    alpha as f32,
                );
            }
            paint_row(row, scene, colors, row_origin, elapsed, width);
            if alpha < 1.0 {
                scene.pop_layer();
            }
            y += row.height * alpha;
        }
        let _ = ctx;
        owes_frame
    }
}

/// Paint one task row's mark, title (with its strike-through) and detail.
fn paint_row(
    row: &TodoRow,
    scene: &mut dyn PaintScene,
    colors: &TodoColors,
    origin: Point,
    elapsed: Duration,
    width: f64,
) {
    let ink = match row.status {
        TodoListStatus::InProgress => colors.ink,
        TodoListStatus::Cancelled => colors.danger,
        _ => colors.muted,
    };
    let mark_origin = Point::new(
        origin.x,
        origin.y
            + (row.height - TODO_ROW_PADDING_Y * 2.0 - TODO_MARK_SIZE).max(0.0) / 2.0
            + TODO_ROW_PADDING_Y,
    );
    paint_mark(row, scene, ink, mark_origin, elapsed);

    let title_ink = match row.status {
        TodoListStatus::Pending => scale_alpha(colors.muted, PENDING_TITLE_ALPHA),
        TodoListStatus::InProgress => scale_alpha(colors.ink, HEADER_TITLE_ALPHA),
        TodoListStatus::Completed => scale_alpha(colors.muted, COMPLETED_TITLE_ALPHA),
        TodoListStatus::Cancelled => scale_alpha(colors.muted, DIM_TITLE_ALPHA),
    };
    let title_x = origin.x + TODO_MARK_SIZE + TODO_GAP;
    let title_y = origin.y + (row.height - row.title.size().height) / 2.0;
    row.title
        .paint(Point::new(title_x, title_y), title_ink, scene);

    // The strike-through: a hairline growing from the title's leading edge.
    if row.status.is_complete() {
        let drawn = strike_at(elapsed);
        if drawn > 0.0 {
            let mid = title_y + row.title.size().height / 2.0;
            scene.fill_rect(
                Point::new(title_x, mid),
                Size::new(row.title_width * drawn, 1.0),
                title_ink,
            );
        }
    }

    if let Some(detail) = &row.detail {
        let detail_x = origin.x + width - TODO_ROW_PADDING_X * 2.0 - detail.size().width;
        detail.paint(
            Point::new(
                detail_x.max(title_x + row.title_width + TODO_GAP),
                origin.y + (row.height - detail.size().height) / 2.0,
            ),
            scale_alpha(colors.muted, DIM_TITLE_ALPHA),
            scene,
        );
    }
}

/// Paint one row's status mark — the four-way morph upstream draws as one SVG.
fn paint_mark(
    row: &TodoRow,
    scene: &mut dyn PaintScene,
    ink: Color,
    origin: Point,
    elapsed: Duration,
) {
    let centre = Point::new(
        origin.x + TODO_MARK_SIZE / 2.0,
        origin.y + TODO_MARK_SIZE / 2.0,
    );
    let ring = Arc::new(
        centre,
        Vec2::new(MARK_RADIUS, MARK_RADIUS),
        0.0,
        std::f64::consts::TAU,
        0.0,
    );
    let fill =
        Ramp::eased(Duration::from_millis(TODO_MARK_FILL_MS), EASE_OUT).progress_clamped(elapsed);
    let was_complete = row.previous.is_complete();
    let fill_alpha = if row.status.is_complete() {
        MARK_FILL_ALPHA * fill as f32
    } else if was_complete {
        MARK_FILL_ALPHA * (1.0 - fill) as f32
    } else {
        0.0
    };
    if fill_alpha > 0.0 {
        scene.fill_path(
            Point::ORIGIN,
            &ring.to_path(style::PATH_TOLERANCE),
            &Brush::Solid(scale_alpha(ink, fill_alpha)),
        );
    }

    // The track: dashed while pending, faint under a running ring, solid
    // otherwise.
    let track_ink = match row.status {
        TodoListStatus::InProgress => scale_alpha(ink, MARK_TRACK_ALPHA),
        _ => ink,
    };
    let track = ring.to_path(style::PATH_TOLERANCE);
    if matches!(row.status, TodoListStatus::Pending) {
        scene.stroke_path_dashed(
            Point::ORIGIN,
            &track,
            MARK_STROKE * 0.75,
            frust::authoring::DashPattern {
                on: 2.0,
                off: 3.0,
                phase: 0.0,
            },
            &Brush::Solid(track_ink),
        );
    } else {
        scene.stroke_path(
            Point::ORIGIN,
            &track,
            MARK_STROKE * 0.75,
            &Brush::Solid(track_ink),
        );
    }

    match row.status {
        TodoListStatus::InProgress => {
            let swept = Ramp::spring(SPRING_LAYOUT).progress_clamped(elapsed) * row.fraction;
            if swept > 0.0 {
                let head = Arc::new(
                    centre,
                    Vec2::new(MARK_RADIUS, MARK_RADIUS),
                    -std::f64::consts::FRAC_PI_2,
                    swept * std::f64::consts::TAU,
                    0.0,
                );
                scene.stroke_path(
                    Point::ORIGIN,
                    &head.to_path(style::PATH_TOLERANCE),
                    MARK_STROKE,
                    &Brush::Solid(ink),
                );
            }
        }
        TodoListStatus::Completed => {
            let drawn = Ramp::eased(Duration::from_millis(TODO_CHECK_MS), EASE_OUT)
                .progress_clamped(elapsed);
            let points: Vec<(f64, f64)> = CHECK_POINTS
                .iter()
                .map(|p| {
                    let (x, y) = mark_point(origin, *p);
                    (x, y)
                })
                .collect();
            scene.stroke_path(
                Point::ORIGIN,
                &drawn_polyline(&points, drawn),
                MARK_STROKE,
                &Brush::Solid(ink),
            );
        }
        TodoListStatus::Cancelled => {
            let drawn = Ramp::eased(Duration::from_millis(TODO_CROSS_MS), EASE_OUT)
                .progress_clamped(elapsed);
            for stroke in CROSS_POINTS {
                let points: Vec<(f64, f64)> =
                    stroke.iter().map(|p| mark_point(origin, *p)).collect();
                scene.stroke_path(
                    Point::ORIGIN,
                    &drawn_polyline(&points, drawn),
                    MARK_STROKE,
                    &Brush::Solid(ink),
                );
            }
        }
        TodoListStatus::Pending => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, DashPattern, PointerButton, PointerEvent};
    use std::any::Any;

    /// The box every list test lays itself into.
    const BOX: Size = Size::new(360.0, 400.0);

    /// Records the primitives the list paints.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rounded: Vec<(Point, Size, Color)>,
        lines: Vec<(Point, Point)>,
        strokes: Vec<f64>,
        dashed: usize,
        fills: usize,
        layers: Vec<f32>,
        clips: usize,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn stroke_line(&mut self, p0: Point, p1: Point, _width: f64, _color: Color) {
            self.lines.push((p0, p1));
        }
        fn stroke_path(&mut self, _origin: Point, path: &BezPath, _width: f64, _brush: &Brush) {
            self.strokes.push(path.elements().len() as f64);
        }
        fn stroke_path_dashed(
            &mut self,
            _origin: Point,
            _path: &BezPath,
            _width: f64,
            _pattern: DashPattern,
            _brush: &Brush,
        ) {
            self.dashed += 1;
        }
        fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {
            self.fills += 1;
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn push_transform(&mut self, _transform: Affine) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// What a header press reports.
    #[derive(Default)]
    struct Opened {
        last: Option<bool>,
        count: u32,
    }

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn plan() -> Vec<TodoListItem> {
        vec![
            todo_item("audit", "Audit the checkout flow").status(TodoListStatus::Completed),
            todo_item("fix", "Close the validation gap")
                .status(TodoListStatus::InProgress)
                .progress(40.0),
            todo_item("ship", "Prepare the release patch").detail("2m"),
        ]
    }

    fn build(view: &TodoListView<Opened>) -> TodoListWidget {
        let mut next_id = 0u64;
        View::<Opened>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn layout(widget: &mut TodoListWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::loose(BOX))
    }

    fn laid_out(view: &TodoListView<Opened>) -> (TodoListWidget, Size) {
        let mut widget = build(view);
        let size = layout(&mut widget);
        (widget, size)
    }

    /// Paint at `ms`, returning what was drawn plus the frame/layout requests.
    fn painted(
        widget: &mut TodoListWidget,
        size: Size,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, at(ms));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame(), ctx.needs_layout())
    }

    fn rebuild(
        widget: &mut TodoListWidget,
        from: &TodoListView<Opened>,
        to: &TodoListView<Opened>,
    ) {
        let mut next_id = 0u64;
        View::<Opened>::rebuild(to, from, widget, &mut BuildCtx::new(&mut next_id));
    }

    fn press(widget: &mut TodoListWidget, size: Size, at_point: Point, state: &mut Opened) {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ORIGIN, size);
            widget.event(
                &mut ctx,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: at_point,
                    button: PointerButton::Primary,
                }),
            );
        }
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Upstream's four statuses, their labels, and the default ring fraction an
    /// `in-progress` task with no percentage shows.
    #[test]
    fn every_status_is_constructible_labelled_and_carries_a_ring_fraction() {
        assert_eq!(TodoListStatus::ALL.len(), 4);
        assert_eq!(TodoListStatus::Pending.label(), "Pending");
        assert_eq!(TodoListStatus::InProgress.label(), "In progress");
        assert_eq!(TodoListStatus::Completed.label(), "Completed");
        assert_eq!(TodoListStatus::Cancelled.label(), "Cancelled");
        assert!(TodoListStatus::Completed.is_complete());
        assert!(!TodoListStatus::Cancelled.is_complete());

        let bare = todo_item("a", "Task");
        assert_eq!(bare.ring_fraction(), TODO_INDETERMINATE);
        assert_eq!(todo_item("a", "Task").progress(40.0).ring_fraction(), 0.4);
        // Upstream clamps its percentage into `0..=100` before normalising.
        assert_eq!(todo_item("a", "Task").progress(240.0).ring_fraction(), 1.0);
        assert_eq!(todo_item("a", "Task").progress(-10.0).ring_fraction(), 0.0);
        assert_eq!(bare.id(), "a");
        assert_eq!(bare.title(), "Task");
    }

    /// A fresh list mounts open, settled and silent: its rows are already
    /// present, so nothing plays and no frame is owed.
    #[test]
    fn a_fresh_list_mounts_open_and_settled() {
        let view = todo_list::<Opened>(plan());
        assert_eq!(view.completed(), 1);
        let (mut widget, size) = laid_out(&view);
        assert!(widget.is_open());
        assert_eq!(widget.row_count(), 3);
        assert_eq!(widget.completed(), 1);
        assert!(size.height > TODO_HEADER_HEIGHT, "the reveal is open");

        let (rec, needs_frame, needs_layout) = painted(&mut widget, size, 0, None);
        assert!(
            !needs_frame && !needs_layout,
            "a settled plan asks for nothing"
        );
        assert_eq!(widget.reveal_progress(), 1.0);
        assert!(rec.clips > 0, "the reveal clips its viewport");
        assert!(!rec.inks.is_empty(), "the header and rows shape text");
    }

    /// The delay is real: the strike-through has not started at
    /// [`TODO_STRIKE_DELAY_MS`] and is fully drawn once its own ramp finishes.
    #[test]
    fn checking_a_task_off_draws_its_strike_through_after_the_delay() {
        // `collapse_on_complete` is off here: completing the only task would
        // otherwise fold the list away before the strike could be seen.
        let before = todo_list::<Opened>(vec![todo_item("ship", "Prepare the patch")])
            .collapse_on_complete(false);
        let (mut widget, size) = laid_out(&before);
        painted(&mut widget, size, 0, None);
        assert_eq!(
            widget.strike_progress(0, at(0)),
            0.0,
            "a pending task is unstruck"
        );

        let after = todo_list::<Opened>(vec![
            todo_item("ship", "Prepare the patch").status(TodoListStatus::Completed),
        ])
        .collapse_on_complete(false);
        rebuild(&mut widget, &before, &after);
        let (_, needs_frame, _) = painted(&mut widget, size, 100, None);
        assert!(needs_frame, "the check-off owes frames");
        assert_eq!(
            widget.strike_progress(0, at(100 + TODO_STRIKE_DELAY_MS)),
            0.0,
            "nothing is drawn until the delay elapses"
        );
        let mid = widget.strike_progress(0, at(100 + TODO_STRIKE_DELAY_MS + TODO_STRIKE_MS / 2));
        assert!(mid > 0.0 && mid < 1.0, "mid-draw: {mid}");
        assert_eq!(
            widget.strike_progress(0, at(100 + TODO_STRIKE_DELAY_MS + TODO_STRIKE_MS)),
            1.0
        );

        // It is painted as a hairline across the title, growing with the draw.
        let (rec, _, _) = painted(
            &mut widget,
            size,
            100 + TODO_STRIKE_DELAY_MS + TODO_STRIKE_MS,
            None,
        );
        let strike = rec
            .rects
            .iter()
            .find(|(_, size, _)| size.height == 1.0)
            .expect("the strike-through is a 1px rule");
        assert!(strike.1.width > 0.0);
    }

    /// Presence, both ways: an appended task enters from zero, and a removed
    /// one keeps its slot until its exit ramp finishes.
    #[test]
    fn an_appended_task_enters_and_a_removed_one_holds_its_slot_through_its_exit() {
        let one = todo_list::<Opened>(vec![todo_item("a", "First")]);
        let (mut widget, size) = laid_out(&one);
        painted(&mut widget, size, 0, None);

        let two = todo_list::<Opened>(vec![todo_item("a", "First"), todo_item("b", "Second")]);
        rebuild(&mut widget, &one, &two);
        assert_eq!(widget.row_count(), 2);
        let (_, needs_frame, needs_layout) = painted(&mut widget, size, 10, None);
        assert!(needs_frame || needs_layout, "the entrance owes a frame");
        assert!(
            widget.row_presence(1, at(10)) < 1.0,
            "the new row starts un-entered"
        );
        painted(&mut widget, size, 5_000, None);
        assert_eq!(widget.row_presence(1, at(5_000)), 1.0, "and settles");

        // Removing it keeps the row mounted for its exit, then closes the slot.
        rebuild(&mut widget, &two, &one);
        assert_eq!(widget.row_count(), 2, "the departing row is still mounted");
        painted(&mut widget, size, 5_010, None);
        assert!(widget.row_presence(1, at(5_010)) > 0.0, "still visible");
        painted(&mut widget, size, 5_010 + TODO_ROW_FADE_MS, None);
        layout(&mut widget);
        assert_eq!(
            widget.row_count(),
            1,
            "the exit has finished and the slot closed"
        );
    }

    /// Upstream's completion effect: an uncontrolled list folds itself away when
    /// every task is done, and comes back when one stops being done.
    #[test]
    fn completing_every_task_collapses_an_uncontrolled_list_and_reopening_one_restores_it() {
        let running = todo_list::<Opened>(vec![
            todo_item("a", "First").status(TodoListStatus::Completed),
            todo_item("b", "Second"),
        ]);
        let (mut widget, size) = laid_out(&running);
        assert!(widget.is_open());

        let done = todo_list::<Opened>(vec![
            todo_item("a", "First").status(TodoListStatus::Completed),
            todo_item("b", "Second").status(TodoListStatus::Completed),
        ]);
        rebuild(&mut widget, &running, &done);
        assert!(!widget.is_open(), "a finished plan folds itself away");

        rebuild(&mut widget, &done, &running);
        assert!(widget.is_open(), "and comes back when work resumes");

        // Opting out keeps it open throughout.
        let keep = todo_list::<Opened>(vec![
            todo_item("a", "First").status(TodoListStatus::Completed),
            todo_item("b", "Second"),
        ])
        .collapse_on_complete(false);
        let keep_done = todo_list::<Opened>(vec![
            todo_item("a", "First").status(TodoListStatus::Completed),
            todo_item("b", "Second").status(TodoListStatus::Completed),
        ])
        .collapse_on_complete(false);
        let (mut widget, _) = laid_out(&keep);
        rebuild(&mut widget, &keep, &keep_done);
        assert!(widget.is_open());
        let _ = size;
    }

    /// A header press toggles an uncontrolled list and reports the requested
    /// state; a controlled one reports and stays where its owner put it.
    #[test]
    fn a_header_press_reports_and_only_an_uncontrolled_list_moves_itself() {
        let view = todo_list::<Opened>(plan()).on_open_change(|state: &mut Opened, open| {
            state.last = Some(open);
            state.count += 1;
        });
        let (mut widget, size) = laid_out(&view);
        let mut state = Opened::default();
        press(
            &mut widget,
            size,
            Point::new(40.0, TODO_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.last, Some(false));
        assert_eq!(state.count, 1);
        assert!(
            !widget.is_open(),
            "an uncontrolled list follows its own press"
        );

        // A press outside the header changes nothing.
        press(
            &mut widget,
            size,
            Point::new(40.0, size.height - 2.0),
            &mut state,
        );
        assert_eq!(state.count, 1);

        let controlled =
            todo_list::<Opened>(plan())
                .open(true)
                .on_open_change(|state: &mut Opened, open| {
                    state.last = Some(open);
                    state.count += 1;
                });
        let (mut widget, size) = laid_out(&controlled);
        let mut state = Opened::default();
        press(
            &mut widget,
            size,
            Point::new(40.0, TODO_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.last, Some(false), "the request is still reported");
        assert!(widget.is_open(), "but a controlled list never moves itself");
    }

    /// The disclosure drives **height**, so a list mid-reveal asks for relayout
    /// rather than a bare repaint, and lands closed.
    #[test]
    fn closing_the_disclosure_drives_layout_and_lands_shut() {
        let open = todo_list::<Opened>(plan());
        let shut = todo_list::<Opened>(plan()).open(false);
        let (mut widget, size) = laid_out(&open);
        painted(&mut widget, size, 0, None);
        let tall = layout(&mut widget);

        rebuild(&mut widget, &open, &shut);
        // The first paint after a retarget only latches the ramp's start.
        let (_, _, needs_layout) = painted(&mut widget, size, 10, None);
        assert!(needs_layout, "a height animation must drive layout");
        painted(&mut widget, size, 10 + TODO_CLOSE_MS / 2, None);
        let mid = layout(&mut widget);
        assert!(mid.height < tall.height && mid.height > TODO_HEADER_HEIGHT);

        painted(&mut widget, size, 10 + TODO_CLOSE_MS, None);
        let closed = layout(&mut widget);
        assert_eq!(closed.height, TODO_HEADER_HEIGHT, "only the header is left");
        assert_eq!(widget.reveal_progress(), 0.0);
    }

    /// `reduce_motion` lands every driver on the first paint: no strike draw, no
    /// reveal ramp, no frames.
    #[test]
    fn reduce_motion_lands_the_reveal_and_the_marks_at_once() {
        let theme = reduced();
        let before = todo_list::<Opened>(vec![todo_item("a", "First")]).collapse_on_complete(false);
        let after = todo_list::<Opened>(vec![
            todo_item("a", "First").status(TodoListStatus::Completed),
        ])
        .collapse_on_complete(false);
        let (mut widget, size) = laid_out(&before);
        painted(&mut widget, size, 0, Some(&theme));
        rebuild(&mut widget, &before, &after);
        let (_, needs_frame, _) = painted(&mut widget, size, 1, Some(&theme));
        assert_eq!(
            widget.strike_progress(0, at(1)),
            1.0,
            "the strike is already drawn"
        );
        assert!(!needs_frame, "and nothing is owed");

        let shut = todo_list::<Opened>(vec![
            todo_item("a", "First").status(TodoListStatus::Completed),
        ])
        .collapse_on_complete(false)
        .open(false);
        rebuild(&mut widget, &after, &shut);
        painted(&mut widget, size, 2, Some(&theme));
        assert_eq!(widget.reveal_progress(), 0.0, "and the reveal snaps shut");
    }

    /// The viewport caps at `maxHeight` and follows the tail of a plan longer
    /// than the cap, which is what keeps the newest task on screen.
    #[test]
    fn the_viewport_caps_at_max_height_and_follows_the_tail() {
        let items: Vec<TodoListItem> = (0..12)
            .map(|index| todo_item(format!("task-{index}"), format!("Task {index}")))
            .collect();
        let view = todo_list::<Opened>(items).max_height(120.0);
        let (mut widget, size) = laid_out(&view);
        painted(&mut widget, size, 0, None);
        layout(&mut widget);
        assert!(widget.content_height() > 120.0, "the plan overflows");
        assert_eq!(
            widget.viewport_box().height(),
            120.0,
            "and the viewport caps"
        );

        painted(&mut widget, size, 5_000, None);
        // The follow offset is negative — the list has glided up to its tail.
        assert!(widget.viewport_box().height() <= 120.0);
    }

    /// Each status paints its own mark: a dashed ring when pending, a swept ring
    /// when running, a drawn check when complete, a drawn cross when cancelled.
    #[test]
    fn each_status_paints_its_own_mark() {
        for status in TodoListStatus::ALL {
            let view = todo_list::<Opened>(vec![todo_item("a", "Task").status(status)]);
            let (mut widget, size) = laid_out(&view);
            let (rec, _, _) = painted(&mut widget, size, 5_000, None);
            match status {
                TodoListStatus::Pending => {
                    assert_eq!(rec.dashed, 1, "a pending ring is dashed");
                }
                TodoListStatus::InProgress => {
                    assert!(
                        rec.dashed == 0 && rec.strokes.len() >= 2,
                        "track plus sweep"
                    );
                }
                TodoListStatus::Completed => {
                    assert!(rec.fills > 0, "a completed mark fills its ring");
                    assert!(rec.strokes.len() >= 2, "and draws the check over it");
                }
                TodoListStatus::Cancelled => {
                    assert!(rec.strokes.len() >= 3, "a cross is two strokes over a ring");
                }
            }
        }
    }

    /// An empty plan says so rather than collapsing to a bare header.
    #[test]
    fn an_empty_plan_paints_its_own_message() {
        let view = todo_list::<Opened>(Vec::new());
        let (mut widget, size) = laid_out(&view);
        assert_eq!(widget.row_count(), 0);
        assert!(size.height > TODO_HEADER_HEIGHT, "the message has room");
        let (rec, _, _) = painted(&mut widget, size, 0, None);
        assert!(!rec.inks.is_empty());
    }

    /// The `pathLength` stand-in: a polyline drawn to a fraction of its own
    /// length, which is how both the check and the cross are animated.
    #[test]
    fn drawn_polyline_draws_a_fraction_of_the_whole_length() {
        let line = [(0.0, 0.0), (10.0, 0.0)];
        assert!(drawn_polyline(&line, 0.0).is_empty(), "nothing at zero");
        let half = drawn_polyline(&line, 0.5);
        assert_eq!(half.elements().len(), 2, "a move and one line");
        assert_eq!(
            half.bounding_box().width(),
            5.0,
            "half the run is half the length"
        );
        assert_eq!(drawn_polyline(&line, 1.0).bounding_box().width(), 10.0);
        // A degenerate run has no length to draw along.
        assert!(drawn_polyline(&[(0.0, 0.0)], 1.0).is_empty());
        assert!(drawn_polyline(&[(1.0, 1.0), (1.0, 1.0)], 1.0).is_empty());
        // Both of upstream's own marks survive the whole sweep.
        for fraction in [0.25, 0.5, 0.75, 1.0] {
            assert!(!drawn_polyline(&CHECK_POINTS, fraction).is_empty());
        }
    }

    /// The strike-through's own timeline: nothing through the delay, then one
    /// eased ramp to fully drawn.
    #[test]
    fn the_strike_timeline_waits_out_its_delay() {
        assert_eq!(strike_at(Duration::ZERO), 0.0);
        assert_eq!(strike_at(Duration::from_millis(TODO_STRIKE_DELAY_MS)), 0.0);
        let mid = strike_at(Duration::from_millis(
            TODO_STRIKE_DELAY_MS + TODO_STRIKE_MS / 2,
        ));
        assert!(mid > 0.0 && mid < 1.0, "mid-draw: {mid}");
        assert_eq!(
            strike_at(Duration::from_millis(TODO_STRIKE_DELAY_MS + TODO_STRIKE_MS)),
            1.0
        );
    }

    /// The palette resolves through the scheme when themed and falls back to
    /// beUI's own light table when not.
    #[test]
    fn the_palette_resolves_through_the_theme_or_falls_back_to_beui_light() {
        let bare = TodoColors::resolve(None);
        assert_eq!(bare.ink, FALLBACK.foreground);
        assert_eq!(bare.success, FALLBACK.success);
        assert_eq!(bare.danger, FALLBACK.danger);

        let theme = crate::theme();
        let themed = TodoColors::resolve(Some(&theme));
        assert_eq!(themed.ink, theme.scheme().on_surface);
        assert_eq!(themed.danger, theme.scheme().error);
        assert_eq!(themed.muted, theme.scheme().on_surface_variant);
    }
}
