//! Ports beUI's `agent-activity` agent-interface part.
//!
//! **Source:** `components/agents/agent-activity/index.tsx` plus its
//! `activity-row.tsx` and `types.ts` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `agent-activity`: *"One adaptive activity stream for reasoning,
//! searches, tool calls, structured execution traces, or a chronological mix."*
//!
//! The live log an agent writes while it works. Two shapes in one component:
//! while the run is **working** the stream is always expanded, a shimmering
//! status line names what is happening, and the content glides upward so the
//! newest row stays in view; once the run is **complete** the stream folds into
//! a one-line summary with a disclosure chevron, derived from what kind of
//! entries the log holds.
//!
//! # Upstream's exports, and where each landed
//!
//! | upstream | here |
//! |---|---|
//! | `AgentActivity` | [`agent_activity`] |
//! | `AgentActivityStatus` | [`AgentActivityStatus`] |
//! | `AgentStepStatus` | [`AgentStepStatus`] |
//! | `AgentActivityItem` union | [`AgentActivityItem`] + [`AgentActivityKind`] |
//! | `AgentActivityContentType` | [`AgentActivityContent`] |
//! | `getContentType` | [`AgentActivityContent::of`] |
//! | `getActiveLabel` | [`AgentActivityContent::active_label`] |
//! | `getSummary` | [`AgentActivityContent::summary`] |
//! | `formatDuration` | [`format_duration`] |
//! | `ThinkingShimmer` working row | [`crate::agents::loading_states`]' [`Shimmer`](crate::agents::loading_states::LoadingStatesVariant::Shimmer) variant, composed as this widget's one child |
//! | `AgentDisclosure` | the reveal — [`ACTIVITY_OPEN_MS`]/[`ACTIVITY_CLOSE_MS`] |
//! | `ActivityRow` | the painted row: kind mark, label, trailing meta |
//!
//! # Motion
//!
//! * **A staggered entrance.** Upstream gives every row the same
//!   `opacity 0.18 / y 6` pair with no offset between them; this port walks them
//!   [`ACTIVITY_STAGGER`] apart through [`Stagger`] so a burst of rows reads as
//!   a cascade — the same widening `animated_sidebar` records for its labels.
//!   `reduce_motion` collapses the offsets back to upstream's single beat.
//! * **A [`Presence`] per row**, so a row that leaves the log plays its exit
//!   before its slot closes.
//! * **The stream glide.** While working, the content is offset by
//!   `min(0, viewport - content)` on [`SPRING_LAYOUT`],
//!   which is what keeps the newest row on the bottom edge instead of jumping.
//! * **The disclosure**, which drives height and therefore asks for relayout
//!   rather than a bare frame while it runs.
//!
//! # Degradations against the web original
//!
//! - **A search row lists no results.** Upstream's `search` item carries a
//!   `results` array, each rendered as its own linked sub-row with a favicon.
//!   A row here is one line — mark, label, trailing meta — so a search shows its
//!   query and its result count as that meta. Nesting a result list needs a
//!   second row shape and a link affordance, which is a component of its own.
//! - **One row shape, not five.** Upstream's `ActivityRow` switches over five
//!   item shapes with per-kind icons from `lucide-react`. This port keeps the
//!   five *kinds* (they drive the mark, the ink and the summary) but paints one
//!   row geometry, with a mark drawn from primitives rather than an icon set.
//! - **No fade mask at the viewport edges.** Upstream masks the capped
//!   viewport with a `linear-gradient` `mask-image`. `PaintScene` has no mask
//!   brush; the viewport clips hard instead.
//! - **The capped viewport does not scroll.** Same posture as
//!   [`crate::agents::todo_list`]: the stream follows its tail, and a reader who
//!   needs to walk back nests it in a scroller.
//! - **Labels are strings.** Upstream takes `ReactNode` for every label, meta
//!   and summary.
//! - **No per-row focus.** The whole log is one widget, so only the summary row
//!   is pressable and there is no per-row focus target to ring.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Shape, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{FrameTime, Theme};
use kurbo::{Arc, Vec2};

use crate::agents::loading_states::{LoadingStatesVariant, loading_states};
use crate::motion::{Presence, PresencePhase, Ramp, Stagger};
use crate::press::{Lane, presses};
use crate::style::{self, scale_alpha};
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};
use crate::tokens::{BEUI_LIGHT, BeuiPalette, sans_family};

/// The status/summary row's height, in logical px (`h-7`).
pub const ACTIVITY_HEADER_HEIGHT: f64 = 28.0;

/// An activity row's smallest height (`min-h-7`).
pub const ACTIVITY_ROW_MIN_HEIGHT: f64 = 28.0;

/// The gap between rows (`space-y-0.5`).
pub const ACTIVITY_ROW_GAP: f64 = 2.0;

/// Vertical padding around the row list (`py-2`).
pub const ACTIVITY_LIST_PADDING_Y: f64 = 8.0;

/// Horizontal padding inside a row (`px-1.5`).
pub const ACTIVITY_ROW_PADDING_X: f64 = 6.0;

/// Vertical padding inside a row (`py-1`).
pub const ACTIVITY_ROW_PADDING_Y: f64 = 4.0;

/// The gap between a row's mark and its label (`gap-2.5`).
pub const ACTIVITY_ROW_GAP_INNER: f64 = 10.0;

/// The gap between the summary and its chevron (`gap-1.5`).
pub const ACTIVITY_HEADER_GAP: f64 = 6.0;

/// A row's mark box (`size-4`).
pub const ACTIVITY_MARK_SIZE: f64 = style::ICON_SIZE;

/// The tallest the stream gets before it starts gliding (`maxHeight = 208`).
pub const ACTIVITY_MAX_HEIGHT: f64 = 208.0;

/// How long the disclosure takes to open, in ms.
pub const ACTIVITY_OPEN_MS: u64 = 220;

/// How long it takes to close, in ms.
pub const ACTIVITY_CLOSE_MS: u64 = 140;

/// How long a row's entrance fade takes, in ms (`opacity: { duration: 0.18 }`).
pub const ACTIVITY_ROW_FADE_MS: u64 = 180;

/// How far a row rises through its entrance, in logical px (`y: 6`).
pub const ACTIVITY_ROW_ENTER_SHIFT: f64 = 6.0;

/// How far a row lifts through its exit, in logical px (`exit: { y: -3 }`).
pub const ACTIVITY_ROW_EXIT_SHIFT: f64 = 3.0;

/// The gap between consecutive rows' entrances — this port's own widening of
/// upstream's single beat, see the [module docs](self)' *Motion*.
pub const ACTIVITY_STAGGER: Duration = Duration::from_millis(35);

/// How long the active step's mark takes to pulse once, in ms
/// (`{ duration: 1.5, repeat: Infinity }`).
pub const ACTIVITY_PULSE_MS: u64 = 1500;

/// The active mark's dimmest and brightest halo alphas
/// (`opacity: [0.35, 0.8, 0.35]`).
const PULSE_LOW: f32 = 0.35;
const PULSE_HIGH: f32 = 0.8;

/// Alpha of a pending row's label (`text-muted-foreground/55`), shared with a
/// row's trailing meta.
const DIM_LABEL_ALPHA: f32 = 0.55;

/// Alpha of a live row's label (`text-foreground/90`).
const LABEL_ALPHA: f32 = 0.9;

/// Alpha of a row mark's ink (`text-muted-foreground/70`).
const MARK_ALPHA: f32 = 0.7;

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// Which phase the run is in — upstream's `AgentActivityStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentActivityStatus {
    /// Live: the stream stays expanded and the status line shimmers.
    #[default]
    Working,
    /// Finished: the stream folds into its summary.
    Complete,
}

/// Where one step stands — upstream's `AgentStepStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentStepStatus {
    /// Not started: an empty ring, dimmed label.
    Pending,
    /// Running: a pulsing dot.
    Active,
    /// Done: a check.
    #[default]
    Complete,
}

/// What kind of entry a row is — the `type` discriminator of upstream's item
/// union.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentActivityKind {
    /// A named step with its own [`AgentStepStatus`].
    #[default]
    Step,
    /// Freeform reasoning text, with no mark of its own.
    Text,
    /// A web search: the label is the query.
    Search,
    /// A tool call: the label is the target, the meta the action.
    Tool,
    /// One entry of a structured execution trace.
    Trace,
}

impl AgentActivityKind {
    /// Every kind, in upstream's own union order.
    pub const ALL: [AgentActivityKind; 5] = [
        AgentActivityKind::Step,
        AgentActivityKind::Text,
        AgentActivityKind::Search,
        AgentActivityKind::Tool,
        AgentActivityKind::Trace,
    ];

    /// Whether this kind paints a leading mark at all — `text` does not
    /// (upstream's `TextRow` is a bare paragraph).
    pub const fn has_mark(self) -> bool {
        !matches!(self, AgentActivityKind::Text)
    }
}

/// What the whole log is made of — upstream's `AgentActivityContentType`, which
/// is one kind when every entry agrees and `mixed` otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentActivityContent {
    /// Every entry is the same kind.
    Uniform(AgentActivityKind),
    /// The entries disagree, or the log is empty and nothing was declared.
    #[default]
    Mixed,
}

impl AgentActivityContent {
    /// Upstream's `getContentType`: the first entry's kind when every entry
    /// matches it, and `mixed` otherwise.
    pub fn of(items: &[AgentActivityItem]) -> Self {
        let Some(first) = items.first() else {
            return AgentActivityContent::Mixed;
        };
        if items.iter().all(|item| item.kind == first.kind) {
            AgentActivityContent::Uniform(first.kind)
        } else {
            AgentActivityContent::Mixed
        }
    }

    /// Upstream's `getActiveLabel`: what the status line says while the run is
    /// live.
    pub const fn active_label(self) -> &'static str {
        match self {
            AgentActivityContent::Uniform(AgentActivityKind::Search) => "Searching the web\u{2026}",
            AgentActivityContent::Uniform(AgentActivityKind::Tool) => "Running tools\u{2026}",
            AgentActivityContent::Uniform(AgentActivityKind::Trace) => {
                "Working through the run\u{2026}"
            }
            AgentActivityContent::Mixed => "Working through it\u{2026}",
            _ => "Thinking\u{2026}",
        }
    }

    /// Upstream's `getSummary`: the one line a finished run folds into.
    pub fn summary(self, items: &[AgentActivityItem], duration: f64) -> String {
        let count = items.len();
        match self {
            AgentActivityContent::Uniform(AgentActivityKind::Step)
            | AgentActivityContent::Uniform(AgentActivityKind::Text) => {
                format!("Thought for {}", format_duration(duration))
            }
            AgentActivityContent::Uniform(AgentActivityKind::Search) => {
                "Searched the web".to_owned()
            }
            AgentActivityContent::Uniform(AgentActivityKind::Tool) => {
                format!("Ran {count} {}", plural(count, "tool", "tools"))
            }
            AgentActivityContent::Uniform(AgentActivityKind::Trace) => {
                // Upstream counts a trace's `thinking`/`message` entries as
                // messages and everything else as a tool call.
                let messages = items
                    .iter()
                    .filter(|item| item.kind == AgentActivityKind::Trace && item.is_message())
                    .count();
                let tools = count - messages;
                format!(
                    "{tools} {}, {messages} {}",
                    plural(tools, "tool call", "tool calls"),
                    plural(messages, "message", "messages")
                )
            }
            AgentActivityContent::Mixed => {
                format!("Completed {count} {}", plural(count, "step", "steps"))
            }
        }
    }
}

/// `singular` when `count` is exactly one, `plural` otherwise — upstream writes
/// this ternary out at each of its four call sites.
fn plural(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

/// Upstream's `formatDuration`: whole seconds under a minute, `Nm` and
/// `Nm Ms` above it, never negative.
pub fn format_duration(duration: f64) -> String {
    let seconds = duration.max(0.0).round() as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if remainder == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m {remainder}s")
    }
}

/// One entry in the log — the port of upstream's five-arm item union.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentActivityItem {
    id: String,
    kind: AgentActivityKind,
    label: String,
    meta: Option<String>,
    status: AgentStepStatus,
    /// Whether a [`Trace`](AgentActivityKind::Trace) entry is a message rather
    /// than a tool call — upstream's `kind: "thinking" | "message"` test.
    message: bool,
}

/// A named step, [`Complete`](AgentStepStatus::Complete) unless told otherwise
/// (upstream's own `item.status ?? "complete"`).
pub fn activity_step(id: impl Into<String>, label: impl Into<String>) -> AgentActivityItem {
    AgentActivityItem::new(id, AgentActivityKind::Step, label)
}

/// A block of freeform reasoning text.
pub fn activity_text(id: impl Into<String>, content: impl Into<String>) -> AgentActivityItem {
    AgentActivityItem::new(id, AgentActivityKind::Text, content)
}

/// A web search, labelled with its query.
pub fn activity_search(id: impl Into<String>, query: impl Into<String>) -> AgentActivityItem {
    AgentActivityItem::new(id, AgentActivityKind::Search, query)
}

/// A tool call against `target`, whose `action` (`read`/`edit`/`run`, or any
/// other verb) becomes the row's trailing meta.
pub fn activity_tool(
    id: impl Into<String>,
    action: impl Into<String>,
    target: impl Into<String>,
) -> AgentActivityItem {
    AgentActivityItem::new(id, AgentActivityKind::Tool, target).meta(action)
}

/// One entry of an execution trace. `message` marks upstream's
/// `thinking`/`message` kinds, which the trace summary counts separately from
/// tool calls.
pub fn activity_trace(
    id: impl Into<String>,
    label: impl Into<String>,
    message: bool,
) -> AgentActivityItem {
    let mut item = AgentActivityItem::new(id, AgentActivityKind::Trace, label);
    item.message = message;
    item
}

impl AgentActivityItem {
    /// The shared constructor behind the five kind helpers.
    fn new(id: impl Into<String>, kind: AgentActivityKind, label: impl Into<String>) -> Self {
        AgentActivityItem {
            id: id.into(),
            kind,
            label: label.into(),
            meta: None,
            status: AgentStepStatus::default(),
            message: false,
        }
    }

    /// Give the entry a trailing metadata string (upstream's `meta`).
    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// Give a [`Step`](AgentActivityKind::Step) its own status.
    pub fn status(mut self, status: AgentStepStatus) -> Self {
        self.status = status;
        self
    }

    /// This entry's identity, which a rebuild diffs against.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What kind of entry this is.
    pub fn kind(&self) -> AgentActivityKind {
        self.kind
    }

    /// This entry's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether a trace entry counts as a message rather than a tool call.
    pub fn is_message(&self) -> bool {
        self.message
    }
}

/// A view-held, typed disclosure callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI agent activity stream. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::agents::agent_activity::{
///     AgentActivityStatus, activity_search, activity_text, activity_tool, agent_activity,
/// };
///
/// let log = agent_activity::<()>(vec![
///     activity_text("reason", "Tracing the checkout submission path."),
///     activity_tool("read", "read", "checkout/submit.ts"),
///     activity_search("search", "order validation failures"),
/// ])
/// .status(AgentActivityStatus::Complete)
/// .duration(6.0);
/// ```
pub struct AgentActivityView<State: 'static> {
    items: Vec<AgentActivityItem>,
    content_type: Option<AgentActivityContent>,
    status: AgentActivityStatus,
    duration: f64,
    open: Option<bool>,
    default_open: bool,
    collapse_on_complete: bool,
    active_label: Option<String>,
    summary: Option<String>,
    max_height: f64,
    on_open_change: Option<OnOpenChange<State>>,
}

/// Create a [`Working`](AgentActivityStatus::Working) stream over `items`.
pub fn agent_activity<State: 'static>(items: Vec<AgentActivityItem>) -> AgentActivityView<State> {
    AgentActivityView {
        items,
        content_type: None,
        status: AgentActivityStatus::default(),
        duration: 0.0,
        open: None,
        default_open: false,
        collapse_on_complete: true,
        active_label: None,
        summary: None,
        max_height: ACTIVITY_MAX_HEIGHT,
        on_open_change: None,
    }
}

impl<State: 'static> AgentActivityView<State> {
    /// Declare what kind of entries to expect before the first one streams in
    /// (`contentType`). Ignored once the log holds anything.
    pub fn content_type(mut self, content: AgentActivityContent) -> Self {
        self.content_type = Some(content);
        self
    }

    /// Report `status` instead of [`Working`](AgentActivityStatus::Working).
    pub fn status(mut self, status: AgentActivityStatus) -> Self {
        self.status = status;
        self
    }

    /// The elapsed run time, in seconds, the step/text summary reads from.
    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration = seconds;
        self
    }

    /// Take ownership of the disclosure. A working run is expanded either way —
    /// upstream's `expanded = working || currentOpen`.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// The disclosure state an uncontrolled stream starts at once it completes
    /// (`defaultOpen`, upstream's own default `false`).
    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Whether finishing folds an uncontrolled stream away
    /// (`collapseOnComplete`).
    pub fn collapse_on_complete(mut self, collapse: bool) -> Self {
        self.collapse_on_complete = collapse;
        self
    }

    /// Override the live status line (`activeLabel`).
    pub fn active_label(mut self, label: impl Into<String>) -> Self {
        self.active_label = Some(label.into());
        self
    }

    /// Override the completed summary (`summary`).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Cap the stream's viewport at `height` logical px (default
    /// [`ACTIVITY_MAX_HEIGHT`]).
    pub fn max_height(mut self, height: f64) -> Self {
        self.max_height = height.max(0.0);
        self
    }

    /// Observe presses on the summary row.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, callback: F) -> Self {
        self.on_open_change = Some(Rc::new(callback));
        self
    }

    /// What this log is made of: its declared kind while empty, else what its
    /// entries agree on.
    pub fn content(&self) -> AgentActivityContent {
        if self.items.is_empty() {
            self.content_type.unwrap_or_default()
        } else {
            AgentActivityContent::of(&self.items)
        }
    }

    /// The live status line this stream resolves to.
    pub fn resolved_active_label(&self) -> String {
        self.active_label
            .clone()
            .unwrap_or_else(|| self.content().active_label().to_owned())
    }

    /// The completed summary this stream resolves to.
    pub fn resolved_summary(&self) -> String {
        self.summary
            .clone()
            .unwrap_or_else(|| self.content().summary(&self.items, self.duration))
    }

    /// The shimmering status line the working row composes — the catalog's own
    /// loading-state primitive rather than a second shimmer authored here.
    fn working_row(&self) -> AnyView<State> {
        any(loading_states()
            .variant(LoadingStatesVariant::Shimmer)
            .label(self.resolved_active_label()))
    }
}

/// The resolved stream palette.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ActivityColors {
    /// Full-strength ink (`text-foreground`).
    ink: Color,
    /// Secondary ink (`text-muted-foreground`).
    muted: Color,
    /// The active mark's halo (`bg-foreground/10`).
    halo: Color,
}

impl ActivityColors {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                ActivityColors {
                    ink: scheme.on_surface,
                    muted: scheme.on_surface_variant,
                    halo: scheme.on_surface,
                }
            }
            None => ActivityColors {
                ink: FALLBACK.foreground,
                muted: FALLBACK.muted_foreground,
                halo: FALLBACK.foreground,
            },
        }
    }
}

/// A row's own text style: `text-sm leading-5`.
fn row_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        ..TextStyle::default()
    }
}

/// The summary row's style: `text-sm font-medium`.
fn summary_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        weight: FontWeight::MEDIUM,
        ..TextStyle::default()
    }
}

/// One retained activity row.
struct ActivityRow {
    id: String,
    kind: AgentActivityKind,
    status: AgentStepStatus,
    label: LabelRun,
    meta: Option<LabelRun>,
    presence: Presence,
    /// How present the row was on the last paint — written by `paint`, read by
    /// `layout`, so an exiting row's slot closes over its exit.
    shown: f64,
    /// The row's resolved height, as of the last layout.
    height: f64,
}

impl ActivityRow {
    /// A fresh row for `item`, closed until something opens it.
    fn new(item: &AgentActivityItem) -> Self {
        ActivityRow {
            id: item.id.clone(),
            kind: item.kind,
            status: item.status,
            label: LabelRun::new(item.label.clone()),
            meta: item.meta.clone().map(LabelRun::new),
            presence: Presence::new(
                Ramp::spring(SPRING_LAYOUT),
                Ramp::eased(Duration::from_millis(ACTIVITY_ROW_FADE_MS), EASE_OUT),
            ),
            shown: 0.0,
            height: ACTIVITY_ROW_MIN_HEIGHT,
        }
    }

    /// Adopt `item`'s content, reporting what changed.
    fn adopt(&mut self, item: &AgentActivityItem) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.label.set_content(item.label.clone()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let meta_changed = match (&mut self.meta, &item.meta) {
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
        if meta_changed {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if self.status != item.status || self.kind != item.kind {
            self.status = item.status;
            self.kind = item.kind;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for an [`AgentActivityView`].
pub struct AgentActivityWidget {
    rows: Vec<ActivityRow>,
    /// The shimmering working row — the composed loading-state primitive.
    working: ChildPod,
    /// The completed summary line.
    summary: LabelRun,
    status: AgentActivityStatus,
    controlled: Option<bool>,
    internal_open: bool,
    collapse_on_complete: bool,
    max_height: f64,
    /// The disclosure, `0` closed .. `1` open.
    reveal: Lane,
    /// The stream's follow offset, `<= 0` while the log overflows.
    follow: Lane,
    /// The entrance cascade, and the frame it started on.
    stagger: Stagger,
    stagger_started: Option<FrameTime>,
    /// How many rows the current cascade covers.
    staggered: usize,
    /// Whether the summary row is held down.
    pressed: bool,
    /// The status/summary row's box, resolved by layout.
    header_box: Rect,
    /// The stream viewport's box, resolved by layout.
    viewport: Rect,
    /// The rows' unclipped height, as of the last layout.
    content_height: f64,
    /// The rows' total height that was laid out, used to detect when
    /// row presence changes need a new layout pass.
    laid_content_height: f64,
    on_open_change: Option<ErasedArgCallback<bool>>,
}

impl AgentActivityWidget {
    /// Whether the stream is showing its rows — always true while working,
    /// upstream's `expanded = working || currentOpen`.
    pub fn is_expanded(&self) -> bool {
        self.status == AgentActivityStatus::Working || self.controlled.unwrap_or(self.internal_open)
    }

    /// Whether the run is still live.
    pub fn is_working(&self) -> bool {
        self.status == AgentActivityStatus::Working
    }

    /// How far the disclosure has opened, `0` closed .. `1` open.
    pub fn reveal_progress(&self) -> f64 {
        self.reveal.value()
    }

    /// How many rows the log holds, including any still playing an exit.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The status/summary row's box, as of the last layout.
    pub fn header_box(&self) -> Rect {
        self.header_box
    }

    /// The stream viewport's box, as of the last layout.
    pub fn viewport_box(&self) -> Rect {
        self.viewport
    }

    /// The rows' unclipped height, as of the last layout.
    pub fn content_height(&self) -> f64 {
        self.content_height
    }

    /// How far the stream has glided to keep its tail in view — `0` while the
    /// log fits, negative once it overflows.
    pub fn stream_offset(&self) -> f64 {
        self.follow.value()
    }

    /// How present row `index` is, `0` gone .. `1` settled.
    pub fn row_presence(&self, index: usize, now: FrameTime) -> f64 {
        self.rows
            .get(index)
            .map_or(0.0, |row| row.presence.presence(now))
    }

    /// How far through its staggered entrance row `index` is at `now`.
    pub fn row_entrance(&self, index: usize, now: FrameTime) -> f64 {
        let Some(started) = self.stagger_started else {
            return 1.0;
        };
        self.stagger
            .revealed(now.saturating_sub(started), index, self.staggered.max(1))
    }

    /// Aim the disclosure at `open` on the ramp its direction is timed by.
    fn aim_reveal(&mut self, open: bool, reduce_motion: bool) {
        let ramp = Ramp::eased(
            Duration::from_millis(if open {
                ACTIVITY_OPEN_MS
            } else {
                ACTIVITY_CLOSE_MS
            }),
            EASE_OUT,
        );
        self.reveal
            .retarget_with(ramp, if open { 1.0 } else { 0.0 });
        if reduce_motion {
            self.reveal.snap();
        }
    }
}

impl<State: 'static> View<State> for AgentActivityView<State> {
    type Element = AgentActivityWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AgentActivityWidget {
        let expanded =
            self.status == AgentActivityStatus::Working || self.open.unwrap_or(self.default_open);
        let mut rows: Vec<ActivityRow> = self.items.iter().map(ActivityRow::new).collect();
        // A stream that mounts with a log already in it does not replay it.
        for row in &mut rows {
            row.presence.set_open(true);
            row.presence.advance(FrameTime::from_nanos(0));
            row.presence.advance(FrameTime::from_nanos(u64::MAX / 2));
            row.shown = 1.0;
        }
        AgentActivityWidget {
            rows,
            working: build_child(&self.working_row(), ctx),
            summary: LabelRun::new(self.resolved_summary()),
            status: self.status,
            controlled: self.open,
            internal_open: self.open.unwrap_or(self.default_open),
            collapse_on_complete: self.collapse_on_complete,
            max_height: self.max_height,
            reveal: Lane::at_rest(
                Ramp::eased(Duration::from_millis(ACTIVITY_OPEN_MS), EASE_OUT),
                if expanded { 1.0 } else { 0.0 },
            ),
            follow: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0),
            stagger: Stagger::new(ACTIVITY_STAGGER, Ramp::spring(SPRING_LAYOUT)),
            stagger_started: None,
            staggered: 0,
            pressed: false,
            header_box: Rect::ZERO,
            viewport: Rect::ZERO,
            content_height: 0.0,
            laid_content_height: 0.0,
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AgentActivityWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.working_row(),
            &self.working_row(),
            &mut element.working,
            ctx,
        );

        if prev.items != self.items {
            let mut kept: Vec<ActivityRow> = Vec::with_capacity(self.items.len());
            let mut previous = std::mem::take(&mut element.rows);
            let mut entered = 0usize;
            for item in &self.items {
                match previous.iter().position(|row| row.id == item.id) {
                    Some(index) => {
                        let mut row = previous.remove(index);
                        flags |= row.adopt(item);
                        if row.presence.set_open(true) {
                            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                        }
                        kept.push(row);
                    }
                    None => {
                        let mut row = ActivityRow::new(item);
                        row.presence.set_open(true);
                        kept.push(row);
                        entered += 1;
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
            }
            for mut gone in previous {
                if gone.presence.is_visible() {
                    gone.presence.set_open(false);
                    kept.push(gone);
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
            element.rows = kept;
            if entered > 0 {
                // A fresh batch restarts the cascade over the whole log, so the
                // newest rows lead and the settled ones sit at rest.
                element.stagger_started = None;
                element.staggered = element.rows.len();
            }
        }

        if element.summary.set_content(self.resolved_summary()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Upstream's completion effect: finishing folds an uncontrolled stream
        // away unless the caller opted out.
        if element.status != self.status {
            if element.status == AgentActivityStatus::Working
                && self.status == AgentActivityStatus::Complete
                && element.controlled.is_none()
            {
                element.internal_open = !self.collapse_on_complete;
            }
            element.status = self.status;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        element.controlled = self.open;
        element.collapse_on_complete = self.collapse_on_complete;
        if element.max_height != self.max_height {
            element.max_height = self.max_height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.open != self.open {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut AgentActivityWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.working_row(), &mut element.working, ctx);
    }
}

impl Widget for AgentActivityWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.rows.retain(|row| row.presence.is_visible());

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        self.header_box =
            Rect::from_origin_size(Point::ORIGIN, Size::new(width, ACTIVITY_HEADER_HEIGHT));

        // The working row is the composed shimmer; it occupies the header slot.
        self.working.layout_child(
            ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(width, ACTIVITY_HEADER_HEIGHT)),
        );
        self.working.set_origin(Point::ORIGIN);
        self.summary.layout(ctx, &summary_style());

        let row_style = row_style();
        let mut content = ACTIVITY_LIST_PADDING_Y * 2.0;
        for row in &mut self.rows {
            if let Some(meta) = &mut row.meta {
                meta.layout(ctx, &row_style);
            }
            let label = row.label.layout(ctx, &row_style);
            row.height = (label.height + ACTIVITY_ROW_PADDING_Y * 2.0).max(ACTIVITY_ROW_MIN_HEIGHT);
            content += (row.height + ACTIVITY_ROW_GAP) * row.shown.clamp(0.0, 1.0);
        }
        content = (content - ACTIVITY_ROW_GAP).max(0.0);
        self.content_height = content;
        self.laid_content_height = content;

        // While working the viewport holds its full cap so the glide has room to
        // run; once complete it shrinks to what the log actually needs.
        let viewport_height = if self.is_working() {
            self.max_height
        } else {
            content.min(self.max_height)
        };
        self.viewport = Rect::from_origin_size(
            Point::new(0.0, ACTIVITY_HEADER_HEIGHT),
            Size::new(width, viewport_height),
        );
        let revealed = viewport_height * self.reveal.value().clamp(0.0, 1.0);
        bc.constrain(Size::new(width, ACTIVITY_HEADER_HEIGHT + revealed.max(0.0)))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, colors) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ActivityColors::resolve(theme),
            )
        };
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();
        let expanded = self.is_expanded();
        self.aim_reveal(expanded, reduce_motion);
        let mut owes_layout = self.reveal.advance(now);
        let mut owes_frame = false;

        if self.is_working() {
            self.working.paint_child(ctx, scene);
        } else {
            self.paint_summary(scene, &colors, origin, size, reduce_motion);
        }

        let revealed = self.reveal.value().clamp(0.0, 1.0);
        if revealed > 0.0 {
            let clip_origin = Point::new(origin.x, origin.y + ACTIVITY_HEADER_HEIGHT);
            let clip_size = Size::new(size.width, (size.height - ACTIVITY_HEADER_HEIGHT).max(0.0));
            scene.push_clip(clip_origin, clip_size);
            scene.push_layer(clip_origin, clip_size, revealed as f32);

            let follow_target = (self.viewport.height() - self.content_height).min(0.0);
            self.follow.retarget(follow_target);
            if reduce_motion {
                self.follow.snap();
            } else {
                owes_frame |= self.follow.advance(now);
            }

            let list_origin = Point::new(
                origin.x,
                origin.y + ACTIVITY_HEADER_HEIGHT + ACTIVITY_LIST_PADDING_Y + self.follow.value(),
            );
            let (rows_owes_frame, rows_owes_layout) =
                self.paint_rows(scene, &colors, list_origin, size.width, reduce_motion, now);
            owes_frame |= rows_owes_frame;
            owes_layout |= rows_owes_layout;
            scene.pop_layer();
            scene.pop_clip();

            // An active step's halo breathes forever, so it asks for the paced
            // cosmetic-loop class rather than an unpaced transition frame.
            if self.has_pulse() && !reduce_motion {
                ctx.request_frame_paced();
            }
        }

        owes_layout |= self.reveal.value() != self.reveal.target();
        if owes_layout {
            ctx.request_layout();
        } else if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.working.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if self.is_working() {
            return route_event_single(&mut self.working, ctx, event);
        }
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
                if hits_header {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                EventResult::Ignored
            }
            PointerPhase::Up if self.pressed => {
                self.pressed = false;
                if hits_header {
                    let next = !self.is_expanded();
                    if self.controlled.is_none() {
                        self.internal_open = next;
                    }
                    if let Some(callback) = &mut self.on_open_change {
                        callback(ctx, next);
                    }
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
        let expanded = self.is_expanded();
        ctx.push_container(
            Role::Group,
            |node| {
                if self.is_working() {
                    node.set_busy();
                } else {
                    node.set_label(self.summary.content());
                    node.add_action(Action::Click);
                    node.set_expanded(expanded);
                }
            },
            |ctx| {
                if self.is_working() {
                    self.working.semantics_child(ctx);
                }
                if !expanded {
                    return;
                }
                for row in self.rows.iter().filter(|r| r.presence.is_visible()) {
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(row.label.content());
                    });
                }
            },
        );
    }

    visit_children!(working);
}

impl AgentActivityWidget {
    /// Paint the completed summary line and its chevron.
    fn paint_summary(
        &self,
        scene: &mut dyn PaintScene,
        colors: &ActivityColors,
        origin: Point,
        size: Size,
        reduce_motion: bool,
    ) {
        let ink = if self.pressed {
            colors.ink
        } else {
            colors.muted
        };
        self.summary.paint(
            Point::new(
                origin.x,
                origin.y + (ACTIVITY_HEADER_HEIGHT - self.summary.size().height) / 2.0,
            ),
            ink,
            scene,
        );

        let turn = if reduce_motion {
            if self.is_expanded() { 1.0 } else { 0.0 }
        } else {
            self.reveal.value().clamp(0.0, 1.0)
        };
        let centre = Point::new(
            origin.x + self.summary.size().width + ACTIVITY_HEADER_GAP + 7.0,
            origin.y + ACTIVITY_HEADER_HEIGHT / 2.0,
        );
        let arm = 6.0;
        let drop = (1.0 - 2.0 * turn) * arm * 0.5;
        scene.stroke_line(
            Point::new(centre.x - arm, centre.y - drop),
            Point::new(centre.x, centre.y + drop),
            1.5,
            scale_alpha(colors.muted, MARK_ALPHA),
        );
        scene.stroke_line(
            Point::new(centre.x, centre.y + drop),
            Point::new(centre.x + arm, centre.y - drop),
            1.5,
            scale_alpha(colors.muted, MARK_ALPHA),
        );
        let _ = size;
    }

    /// Paint the log's rows, returning whether any of them still owes a frame or layout.
    /// Detects when row presence animations change the content height and signals
    /// that layout is needed to update the list's size.
    fn paint_rows(
        &mut self,
        scene: &mut dyn PaintScene,
        colors: &ActivityColors,
        list_origin: Point,
        width: f64,
        reduce_motion: bool,
        now: FrameTime,
    ) -> (bool, bool) {
        if self.rows.is_empty() {
            return (false, false);
        }
        let started = *self.stagger_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let run = if reduce_motion {
            self.stagger.collapsed()
        } else {
            self.stagger
        };
        let count = self.staggered.max(1);
        let mut owes_frame = self.staggered > 0 && !run.is_settled(elapsed, count);

        let mut y = list_origin.y;
        let pulse = pulse_alpha(now, reduce_motion);
        let mut current_content = ACTIVITY_LIST_PADDING_Y * 2.0;
        for (index, row) in self.rows.iter_mut().enumerate() {
            let presence = row.presence.advance(now);
            owes_frame |= row.presence.is_animating();
            let entrance = if self.staggered == 0 {
                1.0
            } else {
                run.revealed(elapsed, index, count)
            };
            let alpha = (presence * entrance).clamp(0.0, 1.0);
            row.shown = presence.clamp(0.0, 1.0);
            // Track content height: the presence animation changes row.shown,
            // which is read by layout to size the list. Request layout whenever
            // this height differs from what was laid out, including the frame
            // the animation settles.
            current_content += (row.height + ACTIVITY_ROW_GAP) * row.shown;
            if alpha <= 0.0 {
                y += row.height * row.shown + ACTIVITY_ROW_GAP;
                continue;
            }
            let shift = match row.presence.phase() {
                PresencePhase::Exiting => -ACTIVITY_ROW_EXIT_SHIFT * (1.0 - alpha),
                _ => ACTIVITY_ROW_ENTER_SHIFT * (1.0 - alpha),
            };
            if alpha < 1.0 {
                scene.push_layer(
                    Point::new(list_origin.x, y),
                    Size::new(width, row.height),
                    alpha as f32,
                );
            }
            paint_row(
                row,
                scene,
                colors,
                Point::new(list_origin.x + ACTIVITY_ROW_PADDING_X, y + shift),
                width,
                pulse,
            );
            if alpha < 1.0 {
                scene.pop_layer();
            }
            y += row.height * row.shown + ACTIVITY_ROW_GAP;
        }
        current_content = (current_content - ACTIVITY_ROW_GAP).max(0.0);
        let owes_layout = (current_content - self.laid_content_height).abs() > 1e-6;
        (owes_frame, owes_layout)
    }

    /// Whether any visible row carries a perpetually pulsing mark.
    fn has_pulse(&self) -> bool {
        self.rows.iter().any(|row| {
            row.presence.is_visible()
                && row.kind == AgentActivityKind::Step
                && row.status == AgentStepStatus::Active
        })
    }
}

/// The active mark's halo alpha at `now` — upstream's
/// `[0.35, 0.8, 0.35]` keyframe loop, frozen at its dimmest under reduced
/// motion.
fn pulse_alpha(now: FrameTime, reduce_motion: bool) -> f32 {
    if reduce_motion {
        return PULSE_LOW;
    }
    let ms = now.as_nanos() as f64 / 1_000_000.0;
    let cycle = ms / ACTIVITY_PULSE_MS as f64;
    let phase = cycle - cycle.floor();
    let triangle = 1.0 - (phase * 2.0 - 1.0).abs();
    PULSE_LOW + (PULSE_HIGH - PULSE_LOW) * triangle as f32
}

/// Paint one row: its kind mark, its label and its trailing meta.
fn paint_row(
    row: &ActivityRow,
    scene: &mut dyn PaintScene,
    colors: &ActivityColors,
    origin: Point,
    width: f64,
    pulse: f32,
) {
    let mark_origin = Point::new(origin.x, origin.y + (row.height - ACTIVITY_MARK_SIZE) / 2.0);
    if row.kind.has_mark() {
        paint_mark(row, scene, colors, mark_origin, pulse);
    }
    let label_x = if row.kind.has_mark() {
        origin.x + ACTIVITY_MARK_SIZE + ACTIVITY_ROW_GAP_INNER
    } else {
        origin.x
    };
    let label_ink = match (row.kind, row.status) {
        (AgentActivityKind::Text, _) => colors.muted,
        (_, AgentStepStatus::Pending) => scale_alpha(colors.muted, DIM_LABEL_ALPHA),
        _ => scale_alpha(colors.ink, LABEL_ALPHA),
    };
    row.label.paint(
        Point::new(
            label_x,
            origin.y + (row.height - row.label.size().height) / 2.0,
        ),
        label_ink,
        scene,
    );

    if let Some(meta) = &row.meta {
        let meta_x = origin.x + width - ACTIVITY_ROW_PADDING_X * 2.0 - meta.size().width;
        meta.paint(
            Point::new(
                meta_x.max(label_x + row.label.size().width + ACTIVITY_ROW_GAP_INNER),
                origin.y + (row.height - meta.size().height) / 2.0,
            ),
            scale_alpha(colors.muted, DIM_LABEL_ALPHA),
            scene,
        );
    }
}

/// Paint a row's leading mark: a check, a pulsing dot or an empty ring for a
/// step, and a kind glyph for the other three.
fn paint_mark(
    row: &ActivityRow,
    scene: &mut dyn PaintScene,
    colors: &ActivityColors,
    origin: Point,
    pulse: f32,
) {
    let ink = scale_alpha(colors.muted, MARK_ALPHA);
    let centre = Point::new(
        origin.x + ACTIVITY_MARK_SIZE / 2.0,
        origin.y + ACTIVITY_MARK_SIZE / 2.0,
    );
    match row.kind {
        AgentActivityKind::Step => match row.status {
            AgentStepStatus::Complete => {
                let mut path = BezPath::new();
                path.move_to(Point::new(origin.x + 3.0, origin.y + 8.5));
                path.line_to(Point::new(origin.x + 6.5, origin.y + 12.0));
                path.line_to(Point::new(origin.x + 13.0, origin.y + 4.5));
                scene.stroke_path(Point::ORIGIN, &path, 1.8, &Brush::Solid(ink));
            }
            AgentStepStatus::Active => {
                // A `size-3` halo behind a `size-1.5` dot, the halo breathing.
                scene.fill_rounded_rect(
                    Point::new(centre.x - 6.0, centre.y - 6.0),
                    Size::new(12.0, 12.0),
                    6.0,
                    scale_alpha(colors.halo, 0.1 * pulse / PULSE_HIGH),
                );
                scene.fill_rounded_rect(
                    Point::new(centre.x - 3.0, centre.y - 3.0),
                    Size::new(6.0, 6.0),
                    3.0,
                    scale_alpha(colors.ink, 0.6),
                );
            }
            AgentStepStatus::Pending => {
                let ring = Arc::new(centre, Vec2::new(5.0, 5.0), 0.0, std::f64::consts::TAU, 0.0);
                scene.stroke_path(
                    Point::ORIGIN,
                    &ring.to_path(style::PATH_TOLERANCE),
                    1.5,
                    &Brush::Solid(ink),
                );
            }
        },
        AgentActivityKind::Search => {
            // A magnifier: a ring with a short handle.
            let ring = Arc::new(
                Point::new(centre.x - 1.0, centre.y - 1.0),
                Vec2::new(4.5, 4.5),
                0.0,
                std::f64::consts::TAU,
                0.0,
            );
            scene.stroke_path(
                Point::ORIGIN,
                &ring.to_path(style::PATH_TOLERANCE),
                1.5,
                &Brush::Solid(ink),
            );
            scene.stroke_line(
                Point::new(centre.x + 2.4, centre.y + 2.4),
                Point::new(centre.x + 5.5, centre.y + 5.5),
                1.5,
                ink,
            );
        }
        AgentActivityKind::Tool => {
            // A terminal box with a prompt caret.
            scene.stroke_path(
                Point::ORIGIN,
                &kurbo::RoundedRect::new(
                    origin.x + 2.0,
                    origin.y + 3.0,
                    origin.x + 14.0,
                    origin.y + 13.0,
                    2.5,
                )
                .to_path(style::PATH_TOLERANCE),
                1.4,
                &Brush::Solid(ink),
            );
            scene.stroke_line(
                Point::new(origin.x + 5.0, origin.y + 6.5),
                Point::new(origin.x + 7.5, origin.y + 8.0),
                1.4,
                ink,
            );
            scene.stroke_line(
                Point::new(origin.x + 7.5, origin.y + 8.0),
                Point::new(origin.x + 5.0, origin.y + 9.5),
                1.4,
                ink,
            );
        }
        AgentActivityKind::Trace => {
            // A stack of three rules — one entry of a run's transcript.
            for step in 0..3 {
                let y = origin.y + 5.0 + step as f64 * 3.0;
                scene.stroke_line(
                    Point::new(origin.x + 3.0, y),
                    Point::new(origin.x + 13.0 - step as f64 * 2.0, y),
                    1.3,
                    ink,
                );
            }
        }
        AgentActivityKind::Text => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, PointerButton, PointerEvent};
    use std::any::Any;

    /// The column every stream test lays itself into.
    const COLUMN: Size = Size::new(420.0, 600.0);

    /// Records the primitives the stream paints.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        strokes: usize,
        lines: usize,
        layers: Vec<f32>,
        clips: usize,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _width: f64, _color: Color) {
            self.lines += 1;
        }
        fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {
            self.strokes += 1;
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

    /// What a summary press reports.
    #[derive(Default)]
    struct Opened {
        last: Option<bool>,
        count: u32,
    }

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn log() -> Vec<AgentActivityItem> {
        vec![
            activity_text("reason", "Tracing the checkout submission path."),
            activity_tool("read", "read", "checkout/submit.ts"),
            activity_search("search", "order validation failures").meta("3 results"),
        ]
    }

    fn build(view: &AgentActivityView<Opened>) -> AgentActivityWidget {
        let mut next_id = 0u64;
        View::<Opened>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn layout(widget: &mut AgentActivityWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::loose(COLUMN))
    }

    fn laid_out(view: &AgentActivityView<Opened>) -> (AgentActivityWidget, Size) {
        let mut widget = build(view);
        let size = layout(&mut widget);
        (widget, size)
    }

    fn painted(
        widget: &mut AgentActivityWidget,
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
        widget: &mut AgentActivityWidget,
        from: &AgentActivityView<Opened>,
        to: &AgentActivityView<Opened>,
    ) {
        let mut next_id = 0u64;
        View::<Opened>::rebuild(to, from, widget, &mut BuildCtx::new(&mut next_id));
    }

    fn press(widget: &mut AgentActivityWidget, size: Size, position: Point, state: &mut Opened) {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ORIGIN, size);
            widget.event(
                &mut ctx,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position,
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

    /// Upstream's `getContentType`: one kind when every entry agrees, `mixed`
    /// when they do not, and `mixed` for an empty log with nothing declared.
    #[test]
    fn the_content_type_is_uniform_only_when_every_entry_agrees() {
        let steps = vec![activity_step("a", "One"), activity_step("b", "Two")];
        assert_eq!(
            AgentActivityContent::of(&steps),
            AgentActivityContent::Uniform(AgentActivityKind::Step)
        );
        assert_eq!(
            AgentActivityContent::of(&log()),
            AgentActivityContent::Mixed
        );
        assert_eq!(AgentActivityContent::of(&[]), AgentActivityContent::Mixed);

        // An empty log falls back to whatever the caller declared.
        let declared = agent_activity::<Opened>(Vec::new())
            .content_type(AgentActivityContent::Uniform(AgentActivityKind::Tool));
        assert_eq!(
            declared.content(),
            AgentActivityContent::Uniform(AgentActivityKind::Tool)
        );
        assert_eq!(declared.resolved_active_label(), "Running tools\u{2026}");
    }

    /// Upstream's `getActiveLabel` and `getSummary`, arm for arm — including
    /// the trace summary's message/tool-call split and every singular form.
    #[test]
    fn the_derived_labels_and_summaries_match_upstreams_own_tables() {
        use AgentActivityContent::{Mixed, Uniform};
        use AgentActivityKind::{Search, Step, Text, Tool, Trace};

        assert_eq!(Uniform(Step).active_label(), "Thinking\u{2026}");
        assert_eq!(Uniform(Text).active_label(), "Thinking\u{2026}");
        assert_eq!(Uniform(Search).active_label(), "Searching the web\u{2026}");
        assert_eq!(Uniform(Tool).active_label(), "Running tools\u{2026}");
        assert_eq!(
            Uniform(Trace).active_label(),
            "Working through the run\u{2026}"
        );
        assert_eq!(Mixed.active_label(), "Working through it\u{2026}");

        let one = vec![activity_tool("a", "run", "tests")];
        let two = vec![
            activity_tool("a", "run", "tests"),
            activity_tool("b", "read", "src"),
        ];
        assert_eq!(Uniform(Tool).summary(&one, 0.0), "Ran 1 tool");
        assert_eq!(Uniform(Tool).summary(&two, 0.0), "Ran 2 tools");
        assert_eq!(Uniform(Step).summary(&two, 6.0), "Thought for 6s");
        assert_eq!(Uniform(Search).summary(&two, 0.0), "Searched the web");
        assert_eq!(Mixed.summary(&one, 0.0), "Completed 1 step");
        assert_eq!(Mixed.summary(&two, 0.0), "Completed 2 steps");

        let trace = vec![
            activity_trace("a", "Thinking it over", true),
            activity_trace("b", "Ran the suite", false),
            activity_trace("c", "Read the file", false),
        ];
        assert_eq!(
            Uniform(Trace).summary(&trace, 0.0),
            "2 tool calls, 1 message"
        );
    }

    /// Upstream's `formatDuration`, including both of its minute forms.
    #[test]
    fn durations_format_the_way_upstream_writes_them() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(6.4), "6s");
        assert_eq!(format_duration(59.0), "59s");
        assert_eq!(format_duration(60.0), "1m");
        assert_eq!(format_duration(90.0), "1m 30s");
        assert_eq!(format_duration(-5.0), "0s", "never negative");
    }

    /// A working run is expanded whatever the disclosure says, shows the
    /// composed shimmer row, and glides its content up to keep the tail in view.
    #[test]
    fn a_working_run_stays_expanded_and_glides_its_tail_into_view() {
        let items: Vec<AgentActivityItem> = (0..14)
            .map(|index| activity_step(format!("s{index}"), format!("Step {index}")))
            .collect();
        let working = agent_activity::<Opened>(items).max_height(120.0);
        let (mut widget, size) = laid_out(&working);
        assert!(widget.is_working() && widget.is_expanded());
        assert_eq!(
            widget.viewport_box().height(),
            120.0,
            "the cap holds the glide"
        );
        assert!(widget.content_height() > 120.0, "and the log overflows it");

        painted(&mut widget, size, 0, None);
        painted(&mut widget, size, 5_000, None);
        assert!(
            widget.stream_offset() < 0.0,
            "the stream glided up: {}",
            widget.stream_offset()
        );

        // Even asked to close, a working run stays open.
        let closed = agent_activity::<Opened>(log()).open(false);
        let (widget, _) = laid_out(&closed);
        assert!(widget.is_expanded(), "`working || currentOpen`");
    }

    /// Completing folds an uncontrolled stream away, and `collapseOnComplete`
    /// off leaves it open.
    #[test]
    fn completing_folds_an_uncontrolled_stream_away() {
        let working = agent_activity::<Opened>(log());
        let (mut widget, _) = laid_out(&working);
        assert!(widget.is_expanded());

        let done = agent_activity::<Opened>(log()).status(AgentActivityStatus::Complete);
        rebuild(&mut widget, &working, &done);
        assert!(!widget.is_expanded(), "a finished run folds away");

        let keep = agent_activity::<Opened>(log());
        let keep_done = agent_activity::<Opened>(log())
            .status(AgentActivityStatus::Complete)
            .collapse_on_complete(false);
        let (mut widget, _) = laid_out(&keep);
        rebuild(&mut widget, &keep, &keep_done);
        assert!(widget.is_expanded(), "unless the caller opted out");
    }

    /// A press on the completed summary reports the requested state and moves
    /// an uncontrolled stream; a controlled one only reports.
    #[test]
    fn a_summary_press_reports_and_only_an_uncontrolled_stream_moves_itself() {
        let done = agent_activity::<Opened>(log())
            .status(AgentActivityStatus::Complete)
            .on_open_change(|state: &mut Opened, open| {
                state.last = Some(open);
                state.count += 1;
            });
        let (mut widget, size) = laid_out(&done);
        assert!(!widget.is_expanded());
        let mut state = Opened::default();
        press(
            &mut widget,
            size,
            Point::new(20.0, ACTIVITY_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.last, Some(true));
        assert!(widget.is_expanded());

        let controlled = agent_activity::<Opened>(log())
            .status(AgentActivityStatus::Complete)
            .open(false)
            .on_open_change(|state: &mut Opened, open| {
                state.last = Some(open);
                state.count += 1;
            });
        let (mut widget, size) = laid_out(&controlled);
        let mut state = Opened::default();
        press(
            &mut widget,
            size,
            Point::new(20.0, ACTIVITY_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.count, 1, "the request is reported");
        assert!(!widget.is_expanded(), "but the owner still decides");
    }

    /// A batch of appended rows cascades in: the first row leads, the last has
    /// not started, and the whole run settles.
    #[test]
    fn an_appended_batch_cascades_in_and_settles() {
        let one = agent_activity::<Opened>(vec![activity_step("a", "First")]);
        let (mut widget, size) = laid_out(&one);
        painted(&mut widget, size, 0, None);

        let three = agent_activity::<Opened>(vec![
            activity_step("a", "First"),
            activity_step("b", "Second"),
            activity_step("c", "Third"),
        ]);
        rebuild(&mut widget, &one, &three);
        assert_eq!(widget.row_count(), 3);
        painted(&mut widget, size, 100, None);
        let lead = widget.row_entrance(0, at(100 + 10));
        let tail = widget.row_entrance(2, at(100 + 10));
        assert!(
            lead > tail,
            "the cascade leads with the first row: {lead} vs {tail}"
        );
        assert_eq!(tail, 0.0, "and the last has not started");

        let (_, needs_frame, needs_layout) = painted(&mut widget, size, 140, None);
        assert!(needs_frame || needs_layout, "a running cascade owes frames");
        painted(&mut widget, size, 6_000, None);
        assert_eq!(widget.row_entrance(2, at(6_000)), 1.0, "and it settles");
    }

    /// A removed row keeps its slot until its exit finishes, then the slot
    /// closes.
    #[test]
    fn a_removed_row_holds_its_slot_through_its_exit() {
        let two = agent_activity::<Opened>(vec![
            activity_step("a", "First"),
            activity_step("b", "Second"),
        ]);
        let one = agent_activity::<Opened>(vec![activity_step("a", "First")]);
        let (mut widget, size) = laid_out(&two);
        painted(&mut widget, size, 0, None);

        rebuild(&mut widget, &two, &one);
        assert_eq!(widget.row_count(), 2, "still mounted for the exit");
        painted(&mut widget, size, 10, None);
        assert!(widget.row_presence(1, at(10)) > 0.0);
        painted(&mut widget, size, 10 + ACTIVITY_ROW_FADE_MS, None);
        layout(&mut widget);
        assert_eq!(widget.row_count(), 1, "the slot closed");
    }

    /// The disclosure drives height, so a closing stream asks for relayout and
    /// lands on the header alone.
    #[test]
    fn the_disclosure_drives_layout_and_lands_on_the_header() {
        let open = agent_activity::<Opened>(log())
            .status(AgentActivityStatus::Complete)
            .open(true);
        let shut = agent_activity::<Opened>(log())
            .status(AgentActivityStatus::Complete)
            .open(false);
        let (mut widget, size) = laid_out(&open);
        painted(&mut widget, size, 0, None);
        let tall = layout(&mut widget);
        assert!(tall.height > ACTIVITY_HEADER_HEIGHT);

        rebuild(&mut widget, &open, &shut);
        let (_, _, needs_layout) = painted(&mut widget, size, 10, None);
        assert!(needs_layout, "a height animation must drive layout");
        painted(&mut widget, size, 10 + ACTIVITY_CLOSE_MS / 2, None);
        let mid = layout(&mut widget);
        assert!(mid.height < tall.height && mid.height > ACTIVITY_HEADER_HEIGHT);

        painted(&mut widget, size, 10 + ACTIVITY_CLOSE_MS, None);
        assert_eq!(layout(&mut widget).height, ACTIVITY_HEADER_HEIGHT);
    }

    /// An active step's halo pulses forever, so the stream asks for the
    /// **paced** cosmetic-loop class; a log with no active step does not.
    #[test]
    fn an_active_step_pulses_on_the_paced_class() {
        // Completed and open, so the pulse is the only thing asking for a
        // frame — a *working* stream's shimmer row would ask on its own.
        let live = agent_activity::<Opened>(vec![
            activity_step("a", "First").status(AgentStepStatus::Complete),
            activity_step("b", "Second").status(AgentStepStatus::Active),
        ])
        .status(AgentActivityStatus::Complete)
        .open(true);
        let (mut widget, size) = laid_out(&live);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, at(0));
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        assert!(
            ctx.needs_frame() && ctx.needs_frame_paced_only(),
            "a pulse is paced"
        );

        let still = agent_activity::<Opened>(vec![
            activity_step("a", "First").status(AgentStepStatus::Complete),
        ])
        .status(AgentActivityStatus::Complete)
        .open(true);
        let (mut widget, size) = laid_out(&still);
        let (_, needs_frame, _) = painted(&mut widget, size, 5_000, None);
        assert!(!needs_frame, "a settled log asks for nothing");

        // The keyframe loop itself: dimmest at the ends, brightest in the middle.
        assert!((pulse_alpha(at(0), false) - PULSE_LOW).abs() < 1e-6);
        assert!((pulse_alpha(at(ACTIVITY_PULSE_MS / 2), false) - PULSE_HIGH).abs() < 1e-6);
        assert_eq!(pulse_alpha(at(ACTIVITY_PULSE_MS / 2), true), PULSE_LOW);
    }

    /// Each kind paints its own mark, and `text` paints none at all.
    #[test]
    fn each_kind_paints_its_own_mark_and_text_paints_none() {
        for kind in AgentActivityKind::ALL {
            let item = match kind {
                AgentActivityKind::Step => activity_step("a", "Step"),
                AgentActivityKind::Text => activity_text("a", "Reasoning"),
                AgentActivityKind::Search => activity_search("a", "query"),
                AgentActivityKind::Tool => activity_tool("a", "run", "tests"),
                AgentActivityKind::Trace => activity_trace("a", "Trace", false),
            };
            let (mut widget, size) = laid_out(&agent_activity::<Opened>(vec![item]));
            let (rec, _, _) = painted(&mut widget, size, 5_000, None);
            let marked = rec.strokes > 0 || rec.lines > 0 || !rec.rounded.is_empty();
            assert_eq!(marked, kind.has_mark(), "{kind:?} mark presence");
            assert!(!rec.inks.is_empty(), "{kind:?} still shapes its label");
        }
    }

    /// `reduce_motion` collapses the cascade to a single beat and lands the
    /// disclosure at once.
    #[test]
    fn reduce_motion_collapses_the_cascade_and_the_reveal() {
        let theme = reduced();
        let one = agent_activity::<Opened>(vec![activity_step("a", "First")]);
        let three = agent_activity::<Opened>(vec![
            activity_step("a", "First"),
            activity_step("b", "Second"),
            activity_step("c", "Third"),
        ]);
        let (mut widget, size) = laid_out(&one);
        painted(&mut widget, size, 0, Some(&theme));
        rebuild(&mut widget, &one, &three);
        painted(&mut widget, size, 100, Some(&theme));
        painted(&mut widget, size, 5_000, Some(&theme));
        // Collapsed, every row shares one ramp, so the last row is as far along
        // as the first.
        let (rec, _, _) = painted(&mut widget, size, 5_000, Some(&theme));
        assert!(!rec.inks.is_empty());
        assert_eq!(widget.row_presence(2, at(5_000)), 1.0);
    }

    /// The stream publishes its one composed child — the shimmer row — so an
    /// inspector sees the loading primitive it is built from.
    #[test]
    fn the_stream_publishes_its_composed_shimmer_row() {
        let (widget, _) = laid_out(&agent_activity::<Opened>(log()));
        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 1);
    }

    /// The palette resolves through the scheme when themed and falls back to
    /// beUI light when not.
    #[test]
    fn the_palette_resolves_through_the_theme_or_falls_back_to_beui_light() {
        let bare = ActivityColors::resolve(None);
        assert_eq!(bare.ink, FALLBACK.foreground);
        assert_eq!(bare.muted, FALLBACK.muted_foreground);

        let theme = crate::theme();
        let themed = ActivityColors::resolve(Some(&theme));
        assert_eq!(themed.ink, theme.scheme().on_surface);
        assert_eq!(themed.muted, theme.scheme().on_surface_variant);
    }

    /// Paint-only stepping: advance paint at regular intervals without layout.
    /// Returns whether layout was requested on the paint pass.
    fn step_paint_only(
        widget: &mut AgentActivityWidget,
        size: Size,
        ms: u64,
        theme: Option<&Theme>,
    ) -> bool {
        let (_, _, needs_layout) = painted(widget, size, ms, theme);
        needs_layout
    }

    /// Appending a row to an open stream requests layout while the entrance runs
    /// and on the frame it settles; after layout, the content height includes
    /// the row. (The widget's own height is not the observable here: while the
    /// stream is working the viewport holds its full cap by design, so only
    /// the content it scrolls grows.)
    #[test]
    fn appending_a_row_to_an_open_stream_requests_layout_during_and_after_entrance() {
        let initial = agent_activity::<Opened>(vec![activity_step("a", "First")])
            .open(true)
            .collapse_on_complete(false);
        let (mut widget, size) = laid_out(&initial);
        layout(&mut widget);
        let initial_content = widget.content_height();
        painted(&mut widget, size, 0, None);

        // Append a new row via rebuild.
        let updated = agent_activity::<Opened>(vec![
            activity_step("a", "First"),
            activity_step("b", "New step"),
        ])
        .open(true)
        .collapse_on_complete(false);
        rebuild(&mut widget, &initial, &updated);

        // Paint-only frames during entrance should request layout at some point.
        let mut layout_requested_during_animation = false;
        for ms in [50u64, 100, 150, 200, 250, 300, 350, 400].iter() {
            if step_paint_only(&mut widget, size, *ms, None) {
                layout_requested_during_animation = true;
                break;
            }
        }
        assert!(
            layout_requested_during_animation,
            "row entrance animation must request layout during entrance"
        );

        // After layout is applied, the content height includes the row (at
        // whatever fraction of its entrance the last paint reached), and the
        // settled row eventually takes its full height.
        layout(&mut widget);
        assert!(
            widget.content_height() > initial_content,
            "new row must increase the content height"
        );
        for ms in [1_000u64, 2_000] {
            step_paint_only(&mut widget, size, ms, None);
        }
        layout(&mut widget);
        let settled = widget.content_height();
        assert!(
            settled > initial_content + ACTIVITY_ROW_MIN_HEIGHT * 0.99,
            "a settled row takes its full height"
        );
        assert!(
            !step_paint_only(&mut widget, size, 3_000, None),
            "a settled stream asks for no more layout"
        );
    }

    /// Removing a row from an open stream requests layout symmetrically.
    #[test]
    fn removing_a_row_from_an_open_stream_requests_layout_during_and_after_exit() {
        let initial = agent_activity::<Opened>(vec![
            activity_step("a", "First"),
            activity_step("b", "To remove"),
        ])
        .open(true)
        .collapse_on_complete(false);
        let (mut widget, size) = laid_out(&initial);
        layout(&mut widget);
        let initial_content = widget.content_height();
        painted(&mut widget, size, 0, None);

        // Remove the row via rebuild.
        let updated = agent_activity::<Opened>(vec![activity_step("a", "First")])
            .open(true)
            .collapse_on_complete(false);
        rebuild(&mut widget, &initial, &updated);

        // Paint-only frames during exit should request layout at some point.
        let mut layout_requested_during_animation = false;
        for ms in [50u64, 100, 150, 200, 250, 300, 350, 400].iter() {
            if step_paint_only(&mut widget, size, *ms, None) {
                layout_requested_during_animation = true;
                break;
            }
        }
        assert!(
            layout_requested_during_animation,
            "row exit animation must request layout during exit"
        );

        // After layout, the content height has started to shrink; once the
        // exit settles the row is gone from the content entirely.
        layout(&mut widget);
        assert!(
            widget.content_height() < initial_content,
            "removed row must decrease the content height"
        );
        for ms in [1_000u64, 2_000] {
            step_paint_only(&mut widget, size, ms, None);
        }
        layout(&mut widget);
        assert!(
            widget.content_height() < initial_content - ACTIVITY_ROW_MIN_HEIGHT * 0.99,
            "a settled exit releases the row's full height"
        );
        assert!(
            !step_paint_only(&mut widget, size, 3_000, None),
            "a settled stream asks for no more layout"
        );
    }

    /// An idle settled stream requests no layout on paint-only frames.
    #[test]
    fn idle_settled_stream_requests_no_layout_on_paint_only_frames() {
        let stream = agent_activity::<Opened>(log())
            .open(true)
            .collapse_on_complete(false);
        let (mut widget, size) = laid_out(&stream);
        painted(&mut widget, size, 0, None);

        // Paint many frames without layout; none should request layout.
        for ms in [10, 20, 50, 100, 200, 500].iter() {
            let needs_layout = step_paint_only(&mut widget, size, *ms, None);
            assert!(
                !needs_layout,
                "settled stream at {}ms should not request layout",
                ms
            );
        }
    }
}
