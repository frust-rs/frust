//! Ports beUI's `file-diff` agent-interface part.
//!
//! **Source:** `components/agents/file-diff.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | trigger `min-h-9 gap-2 py-1` | [`FILE_DIFF_HEADER_HEIGHT`] |
//! | file `font-mono text-xs text-foreground/80` | the mono file run |
//! | `ChangeCount` `+N` / `−N` in emerald / rose | [`FileDiffModel::additions`] / [`FileDiffModel::deletions`], tinted from `--success` / `--destructive` |
//! | spinner while streaming, check when applied | [`FileDiffStatus`] |
//! | `rotate: open ? 180 : 0` chevron | the file reveal drives the rotation |
//! | `AgentDisclosure` clip-path reveal | the file reveal lane, a clipped band |
//! | content `pl-6 pt-1.5` in a `rounded-xl bg-muted/80` | [`FILE_DIFF_INDENT`], [`FILE_DIFF_CONTENT_GAP`], [`FILE_DIFF_PANEL_RADIUS`] |
//! | row grid `2.25rem 2.25rem 1rem minmax(0,1fr)` | [`FILE_DIFF_GUTTER_WIDTH`], [`FILE_DIFF_MARKER_WIDTH`] |
//! | `bg-emerald-500/[0.07]` / `bg-rose-500/[0.07]` | [`FILE_DIFF_ROW_ALPHA`] over the same two hues |
//! | `maxHeight = 220` | [`FILE_DIFF_MAX_HEIGHT`] |
//! | `copyText`/`onCopy` button | [`FileDiffView::on_copy`] (moved into the header — see the degradations) |
//!
//! # The input is a parsed model, never raw diff text
//!
//! Upstream takes an already-flat `FileDiffLine[]`; its previews build that
//! array by hand. This port takes the same shape one level up — a
//! [`FileDiffModel`] of [`DiffHunk`]s of [`FileDiffLine`]s — and **parses
//! nothing**. Turning a `git diff` / unified-diff payload into hunks is a
//! text-processing job with its own rename, binary-file, no-newline-at-EOF and
//! combined-diff edge cases; it belongs to whatever produced the diff, not to a
//! widget, and shipping a half-correct parser inside a design-system component
//! would be worse than shipping none. The hunk layer is the one addition over
//! upstream, and it is what [`FileDiffView::open_hunks`] collapses.
//!
//! # Two reveals, two clocks
//!
//! The file's own disclosure and each hunk's are separate lanes, and both feed
//! the widget's reported height — so `paint` asks for **layout** while either
//! is in flight and once more on the frame a value changed, the same
//! layout-skip guard the catalog's accordion documents. The per-line entrance
//! is a third, paint-only motion: an opening hunk's rows rise and fade in on a
//! shared [`Stagger`], clocked from the frame the hunk started opening, which
//! is upstream's "progressive rows".
//!
//! # Degradations against the web original
//!
//! - **No diff parsing**, above.
//! - **No syntax highlighting by default.** Each line's content is one
//!   monochrome [`CodeSpan`] unless the caller supplies its own
//!   ([`FileDiffLine::colored`]) — the same shiki degradation the
//!   [code block](super::code_block) records, for the same reason.
//! - **No follow-the-tail while streaming.** Upstream scrolls its viewport to
//!   the bottom on every streamed row; there is no scroll container here, so a
//!   diff taller than [`FileDiffView::max_height`] is clipped at the cap.
//! - **The copy affordance sits in the header, not the panel foot.** Upstream
//!   pins it below the rows; here it joins the header's right cluster, so the
//!   panel body stays a pure row grid and the affordance stays reachable while
//!   the file is collapsed.
//! - **`collapseOnComplete` is the caller's.** Upstream opens itself when a
//!   stream starts and closes itself when it completes. This port is
//!   controlled end to end (the catalog's rule), so the app watches the status
//!   and passes the `open` it wants; the component never writes its own.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, Color, ErasedArgCallback, ErasedCallback,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, erase_callback, erase_callback_arg,
};
use frust::{FrameTime, Theme};

use crate::agents::code_block::{
    CODE_LINE_HEIGHT, CODE_TEXT_SIZE, CodePalette, CodeSpan, CodeTokenClass, code_palette,
    code_span, code_style, draw_check, draw_chevron, draw_file_glyph, draw_spinner, spinner_angle,
};
use crate::motion::{Ramp, Stagger};
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style::{self, with_alpha};
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The file header row's height, in logical px (`min-h-9`).
pub const FILE_DIFF_HEADER_HEIGHT: f64 = 36.0;

/// The gap between the header's parts, in logical px (`gap-2`).
pub const FILE_DIFF_HEADER_GAP: f64 = style::GAP_MD;

/// How far the disclosed panel is indented, in logical px (`pl-6`).
pub const FILE_DIFF_INDENT: f64 = 24.0;

/// The gap between the header and the panel, in logical px (`pt-1.5`).
pub const FILE_DIFF_CONTENT_GAP: f64 = 6.0;

/// The panel's corner radius (`rounded-xl`).
pub const FILE_DIFF_PANEL_RADIUS: f64 = style::RADIUS_XL;

/// The panel's vertical padding, in logical px.
pub const FILE_DIFF_PANEL_PADDING_Y: f64 = 6.0;

/// One line-number gutter's width, in logical px (`2.25rem`).
pub const FILE_DIFF_GUTTER_WIDTH: f64 = 36.0;

/// The `+`/`−` marker column's width, in logical px (`1rem`).
pub const FILE_DIFF_MARKER_WIDTH: f64 = 16.0;

/// A hunk header row's height, in logical px.
pub const FILE_DIFF_HUNK_HEADER_HEIGHT: f64 = 24.0;

/// The height cap on the disclosed panel, in logical px (`maxHeight = 220`).
pub const FILE_DIFF_MAX_HEIGHT: f64 = 220.0;

/// Alpha of the wash an added or removed row is tinted with
/// (`bg-emerald-500/[0.07]`, `bg-rose-500/[0.07]`).
pub const FILE_DIFF_ROW_ALPHA: f32 = 0.07;

/// Alpha the gutter numbers are painted at (`text-muted-foreground/40`).
pub const FILE_DIFF_GUTTER_ALPHA: f32 = 0.4;

/// How far apart consecutive rows of an opening hunk start, in logical px of
/// clock — the "progressive rows" cascade.
pub const FILE_DIFF_ROW_STAGGER: Duration = Duration::from_millis(18);

/// How long one row's own entrance takes.
pub const FILE_DIFF_ROW_ENTRANCE: Duration = Duration::from_millis(180);

/// How far a row rises into place, in logical px.
pub const FILE_DIFF_ROW_RISE: f64 = 6.0;

/// The ramp both disclosure lanes play, in both directions.
pub const FILE_DIFF_REVEAL: Ramp = Ramp::spring(SPRING_PANEL);

/// The cascade an opening hunk's rows arrive on.
pub fn file_diff_row_cascade() -> Stagger {
    Stagger::eased(FILE_DIFF_ROW_STAGGER, FILE_DIFF_ROW_ENTRANCE, EASE_OUT)
}

/// Whether the edit is still being applied (`status`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileDiffStatus {
    /// `"streaming"` — rows are still arriving.
    #[default]
    Streaming,
    /// `"complete"` — the edit is applied.
    Complete,
}

impl FileDiffStatus {
    /// Whether rows are still arriving.
    pub const fn is_streaming(self) -> bool {
        matches!(self, FileDiffStatus::Streaming)
    }

    /// The word a screen reader is given for this status
    /// (`aria-label="Applying changes"` / `"Changes applied"`).
    pub const fn label(self) -> &'static str {
        match self {
            FileDiffStatus::Streaming => "Applying changes",
            FileDiffStatus::Complete => "Changes applied",
        }
    }
}

/// What one diff row is (`FileDiffLineType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileDiffLineKind {
    /// An added line — `+`, tinted with the success hue.
    Added,
    /// A removed line — `−`, tinted with the destructive hue.
    Removed,
    /// An unchanged context line.
    #[default]
    Context,
}

impl FileDiffLineKind {
    /// The gutter marker this kind carries (`"+"`, `"−"`, or nothing).
    pub const fn marker(self) -> &'static str {
        match self {
            FileDiffLineKind::Added => "+",
            FileDiffLineKind::Removed => "\u{2212}",
            FileDiffLineKind::Context => "",
        }
    }
}

/// One row of a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiffLine {
    /// What the row is.
    pub kind: FileDiffLineKind,
    /// Its line number in the old file, when it has one.
    pub old_line: Option<u32>,
    /// Its line number in the new file, when it has one.
    pub new_line: Option<u32>,
    /// The row's text.
    pub content: String,
    /// Caller-supplied syntax spans for the row; monochrome when `None`.
    pub spans: Option<Vec<CodeSpan>>,
}

/// Build a context row holding `content`.
pub fn diff_line(kind: FileDiffLineKind, content: impl Into<String>) -> FileDiffLine {
    FileDiffLine {
        kind,
        old_line: None,
        new_line: None,
        content: content.into(),
        spans: None,
    }
}

impl FileDiffLine {
    /// Give the row its old-file line number.
    pub const fn old_line(mut self, line: u32) -> Self {
        self.old_line = Some(line);
        self
    }

    /// Give the row its new-file line number.
    pub const fn new_line(mut self, line: u32) -> Self {
        self.new_line = Some(line);
        self
    }

    /// Give the row caller-supplied syntax spans (see the [module
    /// docs](self)' degradations).
    pub fn colored(mut self, spans: impl Into<Vec<CodeSpan>>) -> Self {
        self.spans = Some(spans.into());
        self
    }

    /// The spans this row paints: the caller's, or one monochrome run.
    fn resolved_spans(&self) -> Vec<CodeSpan> {
        match &self.spans {
            Some(spans) if !spans.is_empty() => spans.clone(),
            _ => vec![code_span(self.content.clone(), CodeTokenClass::Plain)],
        }
    }
}

/// One collapsible run of rows — a unified diff's `@@ … @@` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    /// The hunk's own header line, shown on its collapse trigger.
    pub header: String,
    /// The rows it holds.
    pub lines: Vec<FileDiffLine>,
}

/// Build a hunk headed by `header`.
pub fn diff_hunk(header: impl Into<String>, lines: impl Into<Vec<FileDiffLine>>) -> DiffHunk {
    DiffHunk {
        header: header.into(),
        lines: lines.into(),
    }
}

/// One file's parsed diff — the whole input to [`file_diff`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiffModel {
    /// The path shown in the header.
    pub file: String,
    /// The file's hunks, in order.
    pub hunks: Vec<DiffHunk>,
}

/// Build a diff model for `file`.
pub fn file_diff_model(file: impl Into<String>, hunks: impl Into<Vec<DiffHunk>>) -> FileDiffModel {
    FileDiffModel {
        file: file.into(),
        hunks: hunks.into(),
    }
}

impl FileDiffModel {
    /// How many rows the diff adds.
    pub fn additions(&self) -> usize {
        self.count(FileDiffLineKind::Added)
    }

    /// How many rows the diff removes.
    pub fn deletions(&self) -> usize {
        self.count(FileDiffLineKind::Removed)
    }

    fn count(&self, kind: FileDiffLineKind) -> usize {
        self.hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .filter(|line| line.kind == kind)
            .count()
    }

    /// Every row, in file order — the flat shape upstream takes.
    pub fn lines(&self) -> impl Iterator<Item = &FileDiffLine> {
        self.hunks.iter().flat_map(|hunk| hunk.lines.iter())
    }
}

/// A view-held callback, erased on build.
type OnState<State> = Rc<dyn Fn(&mut State)>;
/// A view-held one-argument callback, erased on build.
type OnArg<State, A> = Rc<dyn Fn(&mut State, A)>;

/// A declarative beUI file diff. See the [module docs](self).
pub struct FileDiffView<State: 'static> {
    model: FileDiffModel,
    status: FileDiffStatus,
    open: bool,
    open_hunks: Vec<bool>,
    max_height: f64,
    on_open_change: Option<OnArg<State, bool>>,
    on_hunk_toggle: Option<OnArg<State, usize>>,
    on_copy: Option<OnState<State>>,
}

/// Create a diff disclosure over `model`, open, with every hunk open.
///
/// **Controlled**: chain [`FileDiffView::open`] /
/// [`FileDiffView::on_open_change`] for the file's own disclosure and
/// [`FileDiffView::open_hunks`] / [`FileDiffView::on_hunk_toggle`] for the
/// per-hunk one.
pub fn file_diff<State: 'static>(model: FileDiffModel) -> FileDiffView<State> {
    FileDiffView {
        model,
        status: FileDiffStatus::default(),
        open: true,
        open_hunks: Vec::new(),
        max_height: FILE_DIFF_MAX_HEIGHT,
        on_open_change: None,
        on_hunk_toggle: None,
        on_copy: None,
    }
}

impl<State: 'static> FileDiffView<State> {
    /// Whether the edit is still being applied (`status`).
    pub fn status(mut self, status: FileDiffStatus) -> Self {
        self.status = status;
        self
    }

    /// Whether the file's rows are disclosed (`open`).
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Which hunks are disclosed, by index. A shorter list (the default, empty)
    /// leaves the remaining hunks open — the same "absent means open" rule the
    /// file-level flag defaults to.
    pub fn open_hunks(mut self, open: impl Into<Vec<bool>>) -> Self {
        self.open_hunks = open.into();
        self
    }

    /// Cap the disclosed panel at `max_height` logical px (`maxHeight`).
    pub fn max_height(mut self, max_height: f64) -> Self {
        self.max_height = max_height.max(0.0);
        self
    }

    /// Report the disclosure a press on the file header asks for.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_change: F) -> Self {
        self.on_open_change = Some(Rc::new(on_change));
        self
    }

    /// Report the hunk a press asks to toggle, by index. Wiring this is what
    /// makes a hunk header a trigger.
    pub fn on_hunk_toggle<F: Fn(&mut State, usize) + 'static>(mut self, on_toggle: F) -> Self {
        self.on_hunk_toggle = Some(Rc::new(on_toggle));
        self
    }

    /// Report a press on the copy affordance. The component never touches a
    /// clipboard; wiring this is what paints the affordance.
    pub fn on_copy<F: Fn(&mut State) + 'static>(mut self, on_copy: F) -> Self {
        self.on_copy = Some(Rc::new(on_copy));
        self
    }

    /// Whether hunk `index` is disclosed.
    fn hunk_open(&self, index: usize) -> bool {
        self.open_hunks.get(index).copied().unwrap_or(true)
    }
}

/// One shaped diff row.
struct RowRuns {
    old: Option<LabelRun>,
    new: Option<LabelRun>,
    marker: LabelRun,
    spans: Vec<(LabelRun, CodeTokenClass)>,
    kind: FileDiffLineKind,
}

/// One retained hunk: its shaped header, its rows, and its own reveal.
struct HunkEntry {
    header: LabelRun,
    rows: Vec<RowRuns>,
    reveal: Lane,
    /// The frame the current opening cascade started on, while one runs.
    cascade_from: Option<FrameTime>,
}

/// Which affordance the roving cursor sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffTarget {
    /// The file header, which toggles the whole disclosure.
    File,
    /// The copy affordance.
    Copy,
    /// One hunk's own header.
    Hunk(usize),
}

/// The retained widget for a [`FileDiffView`].
pub struct FileDiffWidget {
    model: FileDiffModel,
    file: LabelRun,
    additions: LabelRun,
    deletions: LabelRun,
    hunks: Vec<HunkEntry>,
    status: FileDiffStatus,
    open: bool,
    open_hunks: Vec<bool>,
    max_height: f64,
    /// The file's own disclosure.
    reveal: Lane,
    width: f64,
    focused: DiffTarget,
    hovered: Option<DiffTarget>,
    captured: Option<DiffTarget>,
    on_open_change: Option<ErasedArgCallback<bool>>,
    on_hunk_toggle: Option<ErasedArgCallback<usize>>,
    on_copy: Option<ErasedCallback>,
}

/// Build the shape-cache carriers for one hunk's rows.
fn build_rows(hunk: &DiffHunk) -> Vec<RowRuns> {
    hunk.lines
        .iter()
        .map(|line| RowRuns {
            old: line.old_line.map(|n| LabelRun::new(n.to_string())),
            new: line.new_line.map(|n| LabelRun::new(n.to_string())),
            marker: LabelRun::new(line.kind.marker()),
            spans: line
                .resolved_spans()
                .into_iter()
                .map(|span| (LabelRun::new(span.text), span.class))
                .collect(),
            kind: line.kind,
        })
        .collect()
}

/// Build every hunk's retained state, resting where its flag puts it.
fn build_hunks<State: 'static>(view: &FileDiffView<State>) -> Vec<HunkEntry> {
    view.model
        .hunks
        .iter()
        .enumerate()
        .map(|(index, hunk)| HunkEntry {
            header: LabelRun::new(hunk.header.clone()),
            rows: build_rows(hunk),
            reveal: Lane::at_rest(
                FILE_DIFF_REVEAL,
                if view.hunk_open(index) { 1.0 } else { 0.0 },
            ),
            cascade_from: None,
        })
        .collect()
}

impl<State: 'static> View<State> for FileDiffView<State> {
    type Element = FileDiffWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FileDiffWidget {
        FileDiffWidget {
            file: LabelRun::new(self.model.file.clone()),
            additions: LabelRun::new(format!("+{}", self.model.additions())),
            deletions: LabelRun::new(format!("\u{2212}{}", self.model.deletions())),
            hunks: build_hunks(self),
            model: self.model.clone(),
            status: self.status,
            open: self.open,
            open_hunks: self.open_hunks.clone(),
            max_height: self.max_height,
            reveal: Lane::at_rest(FILE_DIFF_REVEAL, if self.open { 1.0 } else { 0.0 }),
            width: 0.0,
            focused: DiffTarget::File,
            hovered: None,
            captured: None,
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
            on_hunk_toggle: self.on_hunk_toggle.as_ref().map(erase_callback_arg),
            on_copy: self.on_copy.as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FileDiffWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        element.on_hunk_toggle = self.on_hunk_toggle.as_ref().map(erase_callback_arg);
        element.on_copy = self.on_copy.as_ref().map(erase_callback);
        let mut flags = ChangeFlags::NONE;

        if prev.model != self.model {
            if element.model.file != self.model.file {
                element.file = LabelRun::new(self.model.file.clone());
            }
            element.additions = LabelRun::new(format!("+{}", self.model.additions()));
            element.deletions = LabelRun::new(format!("\u{2212}{}", self.model.deletions()));
            if element.hunks.len() == self.model.hunks.len() {
                // Same shape: re-shape the rows in place so every hunk keeps the
                // reveal it is flying on.
                for (entry, hunk) in element.hunks.iter_mut().zip(self.model.hunks.iter()) {
                    entry.header = LabelRun::new(hunk.header.clone());
                    entry.rows = build_rows(hunk);
                }
            } else {
                element.hunks = build_hunks(self);
                element.captured = None;
                element.hovered = None;
                element.focused = DiffTarget::File;
            }
            element.model = self.model.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.status != self.status {
            element.status = self.status;
            flags |= ChangeFlags::PAINT;
        }
        if element.max_height != self.max_height {
            element.max_height = self.max_height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open || element.open_hunks != self.open_hunks {
            element.open = self.open;
            element.open_hunks = self.open_hunks.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Retargeted, never restarted: a rebuild re-passing a flag a lane is
        // already flying toward leaves its clock alone.
        element
            .reveal
            .retarget(if element.open { 1.0 } else { 0.0 });
        for index in 0..element.hunks.len() {
            let open = element.hunk_open(index);
            let entry = &mut element.hunks[index];
            let was = entry.reveal.target();
            entry.reveal.retarget(if open { 1.0 } else { 0.0 });
            if entry.reveal.target() != was {
                // An opening hunk re-runs its row cascade; a closing one drops it.
                entry.cascade_from = None;
            }
        }
        flags
    }
}

impl FileDiffWidget {
    /// Whether hunk `index` is disclosed.
    fn hunk_open(&self, index: usize) -> bool {
        self.open_hunks.get(index).copied().unwrap_or(true)
    }

    /// Whether a copy affordance is painted.
    fn copyable(&self) -> bool {
        self.on_copy.is_some()
    }

    /// Whether a hunk header is a trigger.
    fn hunks_collapsible(&self) -> bool {
        self.on_hunk_toggle.is_some()
    }

    /// The file header's box, in widget-local space.
    fn header_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(self.width, FILE_DIFF_HEADER_HEIGHT),
        )
    }

    /// The copy affordance's box, in widget-local space.
    fn copy_rect(&self) -> Option<Rect> {
        if !self.copyable() {
            return None;
        }
        let box_size = style::ICON_SIZE + style::GAP_MD;
        let x = (self.width - box_size).max(0.0);
        Some(Rect::from_origin_size(
            Point::new(x, (FILE_DIFF_HEADER_HEIGHT - box_size) / 2.0),
            Size::new(box_size, box_size),
        ))
    }

    /// Hunk `index`'s row band height right now.
    fn hunk_body(&self, index: usize) -> f64 {
        self.hunks.get(index).map_or(0.0, |entry| {
            (entry.reveal.value().clamp(0.0, 1.0) * entry.rows.len() as f64 * CODE_LINE_HEIGHT)
                .max(0.0)
        })
    }

    /// Hunk `index`'s whole height: its header plus its band.
    fn hunk_height(&self, index: usize) -> f64 {
        FILE_DIFF_HUNK_HEADER_HEIGHT + self.hunk_body(index)
    }

    /// The panel's natural (uncapped) height.
    fn panel_natural(&self) -> f64 {
        (0..self.hunks.len())
            .map(|i| self.hunk_height(i))
            .sum::<f64>()
            + FILE_DIFF_PANEL_PADDING_Y * 2.0
    }

    /// The panel's height after the cap.
    fn panel_height(&self) -> f64 {
        self.panel_natural().min(self.max_height)
    }

    /// The whole disclosed block's height, tracking the file reveal.
    fn content_height(&self) -> f64 {
        (self.reveal.value().clamp(0.0, 1.0) * (FILE_DIFF_CONTENT_GAP + self.panel_height()))
            .max(0.0)
    }

    /// The panel's box, in widget-local space.
    fn panel_rect(&self) -> Rect {
        let top = FILE_DIFF_HEADER_HEIGHT + FILE_DIFF_CONTENT_GAP;
        Rect::from_origin_size(
            Point::new(FILE_DIFF_INDENT, top),
            Size::new(
                (self.width - FILE_DIFF_INDENT).max(0.0),
                self.panel_height(),
            ),
        )
    }

    /// Hunk `index`'s header row, in widget-local space.
    fn hunk_header_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.hunks.len() {
            return None;
        }
        let panel = self.panel_rect();
        let mut y = panel.y0 + FILE_DIFF_PANEL_PADDING_Y;
        for i in 0..index {
            y += self.hunk_height(i);
        }
        Some(Rect::from_origin_size(
            Point::new(panel.x0, y),
            Size::new(panel.width(), FILE_DIFF_HUNK_HEADER_HEIGHT),
        ))
    }

    /// The affordance under a widget-local `pos`, if any.
    fn hit(&self, pos: Point) -> Option<DiffTarget> {
        if let Some(rect) = self.copy_rect()
            && rect.contains(pos)
        {
            return Some(DiffTarget::Copy);
        }
        if self.header_rect().contains(pos) {
            return Some(DiffTarget::File);
        }
        if self.hunks_collapsible() && self.reveal.value() > 0.5 {
            let panel = self.panel_rect();
            for index in 0..self.hunks.len() {
                let Some(rect) = self.hunk_header_rect(index) else {
                    continue;
                };
                if rect.contains(pos) && pos.y < panel.y1 {
                    return Some(DiffTarget::Hunk(index));
                }
            }
        }
        None
    }

    /// The affordances the roving cursor can reach, in visual order.
    fn targets(&self) -> Vec<DiffTarget> {
        let mut targets = vec![DiffTarget::File];
        if self.copyable() {
            targets.push(DiffTarget::Copy);
        }
        if self.hunks_collapsible() && self.open {
            targets.extend((0..self.hunks.len()).map(DiffTarget::Hunk));
        }
        targets
    }

    /// The next affordance `step` places from the focused one, wrapping.
    fn step_target(&self, step: isize) -> Option<DiffTarget> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let at = targets.iter().position(|t| *t == self.focused).unwrap_or(0);
        let next = (at as isize + step).rem_euclid(targets.len() as isize) as usize;
        targets.get(next).copied()
    }

    /// Report the decision `target` asks for. Fires exactly one callback.
    fn activate(&mut self, ctx: &mut EventCtx, target: DiffTarget) {
        match target {
            DiffTarget::File => {
                let next = !self.open;
                if let Some(on_change) = self.on_open_change.as_mut() {
                    on_change(ctx, next);
                }
            }
            DiffTarget::Copy => {
                if let Some(on_copy) = self.on_copy.as_mut() {
                    on_copy(ctx);
                }
            }
            DiffTarget::Hunk(index) => {
                if index < self.hunks.len()
                    && let Some(on_toggle) = self.on_hunk_toggle.as_mut()
                {
                    on_toggle(ctx, index);
                }
            }
        }
    }

    /// The row tint for `kind`, or `None` for a context row.
    fn row_wash(kind: FileDiffLineKind, palette: &CodePalette, danger: Color) -> Option<Color> {
        match kind {
            FileDiffLineKind::Added => Some(with_alpha(palette.string, FILE_DIFF_ROW_ALPHA)),
            FileDiffLineKind::Removed => Some(with_alpha(danger, FILE_DIFF_ROW_ALPHA)),
            FileDiffLineKind::Context => None,
        }
    }
}

impl Widget for FileDiffWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        // Explicit: every run here is code in the mono stack, and `TypeScale`
        // has no monospace role to take it from.
        let code = code_style(CODE_TEXT_SIZE);
        self.file.layout(ctx, &code);
        self.additions.layout(ctx, &code);
        self.deletions.layout(ctx, &code);
        for hunk in &mut self.hunks {
            hunk.header.layout(ctx, &code);
            for row in &mut hunk.rows {
                if let Some(run) = &mut row.old {
                    run.layout(ctx, &code);
                }
                if let Some(run) = &mut row.new {
                    run.layout(ctx, &code);
                }
                row.marker.layout(ctx, &code);
                for (run, _) in &mut row.spans {
                    run.layout(ctx, &code);
                }
            }
        }
        bc.constrain(Size::new(
            self.width,
            FILE_DIFF_HEADER_HEIGHT + self.content_height(),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let tokens = BeuiTokens::resolve(theme);
        let danger = theme.map_or(BEUI_LIGHT.destructive, |t| t.scheme().error);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        // ---- advance every lane ---------------------------------------------
        let before: Vec<f64> = std::iter::once(self.reveal.value())
            .chain(self.hunks.iter().map(|h| h.reveal.value()))
            .collect();
        let mut moving = false;
        if reduce_motion {
            self.reveal.snap();
        } else {
            moving |= self.reveal.advance(now);
        }
        for entry in &mut self.hunks {
            if reduce_motion {
                entry.reveal.snap();
                entry.cascade_from = None;
            } else {
                moving |= entry.reveal.advance(now);
                if entry.reveal.target() >= 1.0 && entry.reveal.value() > 0.0 {
                    entry.cascade_from.get_or_insert(now);
                } else if entry.reveal.target() <= 0.0 {
                    entry.cascade_from = None;
                }
            }
        }

        // ---- the file header -------------------------------------------------
        let mut x = origin.x;
        let icon_y = origin.y + (FILE_DIFF_HEADER_HEIGHT - style::ICON_SIZE) / 2.0;
        draw_file_glyph(
            scene,
            Point::new(x, icon_y),
            style::ICON_SIZE,
            palette.comment,
        );
        x += style::ICON_SIZE + FILE_DIFF_HEADER_GAP;

        let file_size = self.file.size();
        self.file.paint(
            Point::new(
                x,
                origin.y + (FILE_DIFF_HEADER_HEIGHT - file_size.height) / 2.0,
            ),
            palette.plain,
            scene,
        );

        // The right cluster: counts, status mark, copy, chevron.
        let chevron_box = style::ICON_SIZE;
        let mut right = origin.x + size.width;
        right -= chevron_box;
        let chevron_centre = Point::new(
            right + chevron_box / 2.0,
            origin.y + FILE_DIFF_HEADER_HEIGHT / 2.0,
        );
        draw_chevron(
            scene,
            chevron_centre,
            chevron_box,
            self.reveal.value().clamp(0.0, 1.0) * std::f64::consts::PI,
            palette.comment,
        );

        if let Some(rect) = self.copy_rect() {
            let at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let hovered = self.hovered == Some(DiffTarget::Copy);
            if hovered {
                scene.fill_rounded_rect(
                    at,
                    rect.size(),
                    style::RADIUS_MD,
                    with_alpha(palette.plain, style::HOVER_WASH_ALPHA),
                );
            }
            draw_file_glyph(
                scene,
                Point::new(at.x + style::GAP_MD / 2.0, at.y + style::GAP_MD / 2.0),
                style::ICON_SIZE,
                if hovered {
                    palette.plain
                } else {
                    palette.comment
                },
            );
            right = origin.x + rect.x0;
        }

        let mark_box = style::ICON_SIZE * 0.85;
        right -= mark_box + style::GAP_SM;
        let mark_at = Point::new(right, origin.y + (FILE_DIFF_HEADER_HEIGHT - mark_box) / 2.0);
        if self.status.is_streaming() {
            draw_spinner(
                scene,
                mark_at,
                mark_box,
                spinner_angle(now, reduce_motion),
                palette.function,
            );
            if !reduce_motion {
                ctx.request_frame_paced();
            }
        } else {
            draw_check(scene, mark_at, mark_box, tokens.success);
        }

        for (run, ink, present) in [
            (&self.deletions, danger, self.model.deletions() > 0),
            (&self.additions, tokens.success, self.model.additions() > 0),
        ] {
            if !present {
                continue;
            }
            let measured = run.size();
            right -= measured.width + FILE_DIFF_HEADER_GAP;
            run.paint(
                Point::new(
                    right,
                    origin.y + (FILE_DIFF_HEADER_HEIGHT - measured.height) / 2.0,
                ),
                ink,
                scene,
            );
        }

        // ---- the disclosed panel --------------------------------------------
        let content = self.content_height();
        if content > 0.5 {
            let panel = self.panel_rect();
            let band = (content - FILE_DIFF_CONTENT_GAP).max(0.0);
            let at = Point::new(origin.x + panel.x0, origin.y + panel.y0);
            let panel_size = Size::new(panel.width(), band);
            scene.fill_rounded_rect(at, panel_size, FILE_DIFF_PANEL_RADIUS, palette.surface);
            scene.push_clip_rounded(at, panel_size, FILE_DIFF_PANEL_RADIUS);

            let cascade = if reduce_motion {
                file_diff_row_cascade().collapsed()
            } else {
                file_diff_row_cascade()
            };
            let mut running_cascade = false;
            for index in 0..self.hunks.len() {
                let Some(header) = self.hunk_header_rect(index) else {
                    continue;
                };
                if header.y0 > panel.y1 {
                    break;
                }
                let entry = &self.hunks[index];
                let header_at = Point::new(origin.x + header.x0, origin.y + header.y0);
                if self.hovered == Some(DiffTarget::Hunk(index)) {
                    scene.fill_rect(
                        header_at,
                        header.size(),
                        with_alpha(palette.plain, style::HOVER_WASH_ALPHA),
                    );
                }
                let measured = entry.header.size();
                entry.header.paint(
                    Point::new(
                        header_at.x + style::GAP_MD,
                        header_at.y + (FILE_DIFF_HUNK_HEADER_HEIGHT - measured.height) / 2.0,
                    ),
                    with_alpha(palette.comment, CodePalette::HUNK_HEADER_ALPHA),
                    scene,
                );
                if self.hunks_collapsible() {
                    draw_chevron(
                        scene,
                        Point::new(
                            header_at.x + header.width() - style::GAP_MD - style::ICON_SIZE / 2.0,
                            header_at.y + FILE_DIFF_HUNK_HEADER_HEIGHT / 2.0,
                        ),
                        style::ICON_SIZE,
                        entry.reveal.value().clamp(0.0, 1.0) * std::f64::consts::PI,
                        palette.comment,
                    );
                }

                let body = self.hunk_body(index);
                if body <= 0.5 {
                    continue;
                }
                let body_top = header_at.y + FILE_DIFF_HUNK_HEADER_HEIGHT;
                scene.push_clip(
                    Point::new(header_at.x, body_top),
                    Size::new(header.width(), body),
                );
                let count = entry.rows.len();
                let elapsed = entry
                    .cascade_from
                    .map_or(Duration::ZERO, |from| now.saturating_sub(from));
                if entry.cascade_from.is_some() && !cascade.is_settled(elapsed, count) {
                    running_cascade = true;
                }
                for (row_index, row) in entry.rows.iter().enumerate() {
                    let arrived = if entry.cascade_from.is_none() {
                        1.0
                    } else {
                        cascade.progress_clamped(elapsed, row_index, count)
                    };
                    let row_top = body_top
                        + row_index as f64 * CODE_LINE_HEIGHT
                        + (1.0 - arrived) * FILE_DIFF_ROW_RISE;
                    if row_top > body_top + body {
                        break;
                    }
                    let row_size = Size::new(header.width(), CODE_LINE_HEIGHT);
                    if let Some(wash) = Self::row_wash(row.kind, &palette, danger) {
                        scene.fill_rect(Point::new(header_at.x, row_top), row_size, wash);
                    }
                    if arrived < 1.0 {
                        scene.push_layer(
                            Point::new(header_at.x, row_top),
                            row_size,
                            arrived as f32,
                        );
                    }
                    let mut column = header_at.x;
                    for gutter in [&row.old, &row.new] {
                        if let Some(run) = gutter {
                            let measured = run.size();
                            run.paint(
                                Point::new(
                                    column + FILE_DIFF_GUTTER_WIDTH
                                        - style::GAP_MD
                                        - measured.width,
                                    row_top + (CODE_LINE_HEIGHT - measured.height) / 2.0,
                                ),
                                with_alpha(palette.comment, FILE_DIFF_GUTTER_ALPHA),
                                scene,
                            );
                        }
                        column += FILE_DIFF_GUTTER_WIDTH;
                    }
                    let marker_ink = match row.kind {
                        FileDiffLineKind::Added => tokens.success,
                        FileDiffLineKind::Removed => danger,
                        FileDiffLineKind::Context => palette.comment,
                    };
                    let marker_size = row.marker.size();
                    row.marker.paint(
                        Point::new(
                            column + (FILE_DIFF_MARKER_WIDTH - marker_size.width) / 2.0,
                            row_top + (CODE_LINE_HEIGHT - marker_size.height) / 2.0,
                        ),
                        marker_ink,
                        scene,
                    );
                    column += FILE_DIFF_MARKER_WIDTH + style::GAP_SM;
                    for (run, class) in &row.spans {
                        let measured = run.size();
                        run.paint(
                            Point::new(
                                column,
                                row_top + (CODE_LINE_HEIGHT - measured.height) / 2.0,
                            ),
                            palette.ink(*class),
                            scene,
                        );
                        column += measured.width;
                    }
                    if arrived < 1.0 {
                        scene.pop_layer();
                    }
                }
                scene.pop_clip();
            }
            scene.pop_clip();
            if running_cascade {
                ctx.request_frame();
            }
        }

        // Every lane feeds the reported height, so a bare frame request would
        // let a disclosure freeze on the intra-frame layout skip. Ask for layout
        // while one moves, and once more on the frame a value changed — which
        // covers the landing frame and the `reduce_motion` snap.
        let after: Vec<f64> = std::iter::once(self.reveal.value())
            .chain(self.hunks.iter().map(|h| h.reveal.value()))
            .collect();
        if moving || after != before {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowDown) => Some(1),
                    Key::Named(NamedKey::ArrowUp) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    let Some(next) = self.step_target(step) else {
                        return EventResult::Ignored;
                    };
                    self.focused = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) {
                    let target = self.focused;
                    if !self.targets().contains(&target) {
                        return EventResult::Ignored;
                    }
                    self.activate(ctx, target);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, ctx.size()) {
                        return EventResult::Ignored;
                    }
                    let Some(target) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(target);
                    self.focused = target;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_some() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = self.hit(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if self.hit(p.position) == Some(armed) {
                        self.activate(ctx, armed);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = format!(
            "{} — +{} \u{2212}{}, {}",
            self.model.file,
            self.model.additions(),
            self.model.deletions(),
            self.status.label()
        );
        ctx.push_node(Role::Button, |node| {
            node.set_label(label.as_str());
            node.set_expanded(self.open);
            node.add_action(Action::Click);
        });
        if self.copyable() {
            ctx.push_node(Role::Button, |node| {
                node.set_label("Copy diff");
                node.add_action(Action::Click);
            });
        }
        if self.open {
            for (index, hunk) in self.model.hunks.iter().enumerate() {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(hunk.header.as_str());
                    node.set_expanded(self.hunk_open(index));
                    if self.hunks_collapsible() {
                        node.add_action(Action::Click);
                    }
                });
            }
        }
    }
}

impl CodePalette {
    /// Alpha a hunk's `@@` header is painted at.
    const HUNK_HEADER_ALPHA: f32 = 0.75;
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        Affine, BezPath, Brush, CornerRadii, KeyEvent, Modifiers, PointerButton, PointerEvent,
    };
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rounded: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _r: f64, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, _r: CornerRadii, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, _r: f64) {
            self.clips.push((o, s));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Toggled {
        open: Option<bool>,
        open_calls: u32,
        hunk: Option<usize>,
        hunk_calls: u32,
        copies: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn model() -> FileDiffModel {
        file_diff_model(
            "src/agents/mod.rs",
            vec![
                diff_hunk(
                    "@@ -1,4 +1,5 @@",
                    vec![
                        diff_line(FileDiffLineKind::Context, "pub mod message;")
                            .old_line(1)
                            .new_line(1),
                        diff_line(FileDiffLineKind::Removed, "pub mod old;").old_line(2),
                        diff_line(FileDiffLineKind::Added, "pub mod code_block;").new_line(2),
                        diff_line(FileDiffLineKind::Added, "pub mod file_diff;").new_line(3),
                    ],
                ),
                diff_hunk(
                    "@@ -40,2 +41,2 @@",
                    vec![
                        diff_line(FileDiffLineKind::Removed, "let old = 1;").old_line(40),
                        diff_line(FileDiffLineKind::Added, "let new = 2;").new_line(41),
                    ],
                ),
            ],
        )
    }

    fn view(open: bool, hunks: Vec<bool>) -> FileDiffView<Toggled> {
        file_diff::<Toggled>(model())
            .open(open)
            .open_hunks(hunks)
            .on_open_change(|s: &mut Toggled, next: bool| {
                s.open = Some(next);
                s.open_calls += 1;
            })
            .on_hunk_toggle(|s: &mut Toggled, index: usize| {
                s.hunk = Some(index);
                s.hunk_calls += 1;
            })
            .on_copy(|s: &mut Toggled| s.copies += 1)
    }

    fn build(v: &FileDiffView<Toggled>) -> FileDiffWidget {
        let mut counter = 0u64;
        View::<Toggled>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut FileDiffWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(460.0, 900.0)),
        )
    }

    fn laid_out(open: bool, hunks: Vec<bool>) -> (FileDiffWidget, Size) {
        let mut w = build(&view(open, hunks));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut FileDiffWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn rebuild(w: &mut FileDiffWidget, from: (bool, Vec<bool>), to: (bool, Vec<bool>)) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Toggled>::rebuild(&view(to.0, to.1), &view(from.0, from.1), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut FileDiffWidget, size: Size, event: &InputEvent, state: &mut Toggled) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn press(w: &mut FileDiffWidget, size: Size, at: Point, state: &mut Toggled) {
        dispatch(w, size, &pointer(PointerPhase::Down, at), state);
        dispatch(w, size, &pointer(PointerPhase::Up, at), state);
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// The model counts its own changes, flattens back to upstream's row list,
    /// and every row kind carries its marker.
    #[test]
    fn the_diff_model_counts_its_changes_and_flattens_to_rows() {
        let model = model();
        assert_eq!(model.additions(), 3);
        assert_eq!(model.deletions(), 2);
        assert_eq!(model.lines().count(), 6);
        assert_eq!(model.hunks.len(), 2);
        assert_eq!(FileDiffLineKind::Added.marker(), "+");
        assert_eq!(FileDiffLineKind::Removed.marker(), "\u{2212}");
        assert_eq!(FileDiffLineKind::Context.marker(), "");
        assert_eq!(FileDiffModel::default().additions(), 0);
    }

    /// A row is monochrome unless the caller colors it, and an empty span list
    /// falls back to the monochrome run rather than painting nothing.
    #[test]
    fn a_row_is_monochrome_unless_the_caller_colors_it() {
        let plain = diff_line(FileDiffLineKind::Added, "let x = 1;");
        let spans = plain.resolved_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].class, CodeTokenClass::Plain);
        assert_eq!(spans[0].text, "let x = 1;");

        let colored = plain.clone().colored(vec![
            code_span("let", CodeTokenClass::Keyword),
            code_span(" x = 1;", CodeTokenClass::Plain),
        ]);
        assert_eq!(colored.resolved_spans().len(), 2);

        let empty = plain.colored(Vec::new());
        assert_eq!(empty.resolved_spans().len(), 1, "an empty list falls back");
    }

    /// The whole chrome paints: the file glyph and name, both counts, the
    /// status mark, the panel, one hunk header per hunk, and every row's tint.
    #[test]
    fn an_open_diff_paints_its_header_panel_and_tinted_rows() {
        let (mut w, size) = laid_out(true, Vec::new());
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert!(!rec.rounded.is_empty(), "the panel surface is filled");
        assert_eq!(rec.rounded[0].2, code_palette(None).surface);
        // The panel clip plus one body clip per hunk.
        assert_eq!(rec.clips.len(), 3);
        // The streaming spinner, the file chevron, and one per hunk header.
        assert_eq!(rec.transforms.len(), 4);
        // Five tinted rows (three added, two removed) and no context tint.
        let tinted = rec
            .rects
            .iter()
            .filter(|(_, _, c)| c.components[3] == FILE_DIFF_ROW_ALPHA)
            .count();
        assert_eq!(tinted, 5);
        assert!(rec.inks.len() > 15, "painted {} runs", rec.inks.len());
    }

    /// A closed diff paints its header and nothing else, and reports the height
    /// of the header alone.
    #[test]
    fn a_closed_diff_is_just_its_header() {
        let (mut w, size) = laid_out(false, Vec::new());
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert!(rec.clips.is_empty(), "no panel, so nothing to clip");
        assert_eq!(layout(&mut w).height, FILE_DIFF_HEADER_HEIGHT);
        assert_eq!(
            rec.transforms.len(),
            2,
            "only the streaming spinner and the file chevron"
        );
    }

    /// Closing the file morphs the widget's own height and every moving frame
    /// asks for relayout — the layout-skip hazard a lane-driven height carries.
    #[test]
    fn closing_the_file_morphs_the_height_and_asks_for_relayout() {
        let (mut w, size) = laid_out(true, Vec::new());
        let open_height = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);
        assert!(open_height > FILE_DIFF_HEADER_HEIGHT);

        rebuild(&mut w, (true, Vec::new()), (false, Vec::new()));
        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a closing disclosure must ask for relayout");

        let mut shortest = open_height;
        for step in 1..=40 {
            paint_at(&mut w, size, None, 100.0 + f64::from(step) * 25.0);
            shortest = shortest.min(layout(&mut w).height);
        }
        assert!(shortest < open_height, "the disclosure never shrank");
        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(layout(&mut w).height, FILE_DIFF_HEADER_HEIGHT);
        let (_, _, still) = paint_at(&mut w, size, None, 5_100.0);
        assert!(!still, "a settled disclosure asks for no more layout");
    }

    /// A hunk collapses on its own lane, leaving the rest of the file alone.
    #[test]
    fn one_hunk_collapses_without_touching_the_others() {
        let (mut w, size) = laid_out(true, Vec::new());
        paint_at(&mut w, size, None, 0.0);
        let both_open = w.panel_natural();

        rebuild(&mut w, (true, Vec::new()), (true, vec![false, true]));
        assert_eq!(w.hunks[0].reveal.target(), 0.0);
        assert_eq!(w.hunks[1].reveal.target(), 1.0);
        for step in 0..=60 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        assert_eq!(w.hunks[0].reveal.value(), 0.0);
        assert_eq!(w.hunks[1].reveal.value(), 1.0);
        assert!(w.panel_natural() < both_open, "the panel never shrank");
        assert_eq!(
            w.hunk_height(0),
            FILE_DIFF_HUNK_HEADER_HEIGHT,
            "a collapsed hunk is its header alone"
        );
    }

    /// Re-passing the flags already being flown toward must not restart either
    /// lane.
    #[test]
    fn a_redundant_rebuild_does_not_restart_a_reveal() {
        let (mut w, size) = laid_out(true, Vec::new());
        paint_at(&mut w, size, None, 0.0);
        rebuild(&mut w, (true, Vec::new()), (false, Vec::new()));
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 160.0);
        let mid = w.reveal.value();
        rebuild(&mut w, (false, Vec::new()), (false, Vec::new()));
        paint_at(&mut w, size, None, 200.0);
        assert!(
            w.reveal.value() < mid,
            "the clock kept running: {mid} -> {}",
            w.reveal.value()
        );
    }

    /// An opening hunk's rows cascade in: early frames composite partial
    /// alphas and ask for another frame, and the cascade settles.
    #[test]
    fn an_opening_hunk_cascades_its_rows_in() {
        let (mut w, size) = laid_out(true, vec![false, false]);
        for step in 0..=60 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        rebuild(
            &mut w,
            (true, vec![false, false]),
            (true, vec![true, false]),
        );
        // The cascade clock starts on the first frame the reveal has actually
        // left zero, so the staged rows show up over the next few frames.
        let mut staged = false;
        for step in 0..=30 {
            let (rec, _, _) = paint_at(&mut w, size, None, 1_600.0 + f64::from(step) * 20.0);
            staged |= rec.layers.iter().any(|a| *a > 0.0 && *a < 1.0);
        }
        assert!(staged, "no row was ever partway in");

        for step in 0..=60 {
            paint_at(&mut w, size, None, 2_300.0 + f64::from(step) * 25.0);
        }
        let (rec, _, _) = paint_at(&mut w, size, None, 5_000.0);
        assert!(
            rec.layers.is_empty(),
            "a settled cascade composites nothing: {:?}",
            rec.layers
        );
    }

    /// `reduce_motion` lands both lanes and the row cascade on the frame they
    /// are painted.
    #[test]
    fn reduce_motion_lands_every_reveal_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(true, Vec::new());
        paint_at(&mut w, size, Some(&theme), 0.0);
        rebuild(&mut w, (true, Vec::new()), (false, vec![false, false]));
        let (rec, _, needs_layout) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(needs_layout, "the snap still publishes its new height");
        assert_eq!(w.reveal.value(), 0.0);
        assert!(w.hunks.iter().all(|h| h.reveal.value() == 0.0));
        assert!(rec.layers.is_empty(), "no staged rows under reduced motion");
    }

    /// Every trigger reports the decision it wants exactly once, and the widget
    /// never writes its own flags.
    #[test]
    fn every_trigger_reports_once_and_writes_nothing() {
        let (mut w, size) = laid_out(true, Vec::new());
        paint_at(&mut w, size, None, 0.0);
        let mut state = Toggled::default();

        press(
            &mut w,
            size,
            Point::new(80.0, FILE_DIFF_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.open, Some(false));
        assert_eq!(state.open_calls, 1);
        assert!(w.open, "the widget never writes its own disclosure");

        let hunk = w.hunk_header_rect(1).unwrap();
        press(&mut w, size, hunk.center(), &mut state);
        assert_eq!(state.hunk, Some(1));
        assert_eq!(state.hunk_calls, 1);

        let copy = w.copy_rect().unwrap();
        press(&mut w, size, copy.center(), &mut state);
        assert_eq!(state.copies, 1);
    }

    /// A press released off its armed target fires nothing — the up-inside
    /// rule.
    #[test]
    fn a_press_released_off_target_reports_nothing() {
        let (mut w, size) = laid_out(true, Vec::new());
        paint_at(&mut w, size, None, 0.0);
        let mut state = Toggled::default();
        let copy = w.copy_rect().unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, copy), &mut state);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(10.0, copy.y)),
            &mut state,
        );
        assert_eq!(state.copies, 0);
        assert_eq!(state.open_calls, 0, "the release did not fire the header");
    }

    /// Keyboard: the arrows rove the file header, the copy affordance and each
    /// hunk header, and the activation keys fire the one under the cursor.
    #[test]
    fn the_arrows_rove_every_trigger_and_enter_fires_it() {
        let (mut w, size) = laid_out(true, Vec::new());
        paint_at(&mut w, size, None, 0.0);
        let mut state = Toggled::default();
        assert_eq!(w.targets().len(), 4, "file, copy and two hunks");

        dispatch(&mut w, size, &key(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, DiffTarget::Copy);
        dispatch(&mut w, size, &key(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, DiffTarget::Hunk(0));
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.hunk, Some(0));
        dispatch(&mut w, size, &key(NamedKey::ArrowUp), &mut state);
        assert_eq!(w.focused, DiffTarget::Copy);
    }

    /// A streaming diff turns its spinner off a paced tick; a complete one asks
    /// for nothing once its lanes have settled.
    #[test]
    fn a_streaming_diff_paces_its_spinner_and_a_settled_one_is_quiet() {
        let mut w = build(&view(true, Vec::new()).status(FileDiffStatus::Streaming));
        let size = layout(&mut w);
        for step in 0..=60 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        let (_, needs_frame, _) = paint_at(&mut w, size, None, 2_000.0);
        assert!(needs_frame, "the spinner asks for its next tick");
        assert_eq!(FileDiffStatus::Streaming.label(), "Applying changes");

        let mut done = build(&view(true, Vec::new()).status(FileDiffStatus::Complete));
        let size = layout(&mut done);
        for step in 0..=60 {
            paint_at(&mut done, size, None, f64::from(step) * 25.0);
        }
        let (_, needs_frame, needs_layout) = paint_at(&mut done, size, None, 3_000.0);
        assert!(!needs_frame && !needs_layout, "a settled diff is quiet");
        assert_eq!(FileDiffStatus::Complete.label(), "Changes applied");
    }

    /// A model swap that keeps the hunk count re-shapes in place and leaves the
    /// reveals alone; one that changes it rebuilds them.
    #[test]
    fn a_model_swap_keeps_the_reveals_when_the_shape_holds() {
        let (mut w, size) = laid_out(true, vec![false, true]);
        for step in 0..=60 {
            paint_at(&mut w, size, None, f64::from(step) * 25.0);
        }
        assert_eq!(w.hunks[0].reveal.value(), 0.0);

        let mut renamed = model();
        renamed.file = "src/agents/file_diff.rs".to_owned();
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let next = file_diff::<Toggled>(renamed)
            .open(true)
            .open_hunks(vec![false, true])
            .on_hunk_toggle(|s: &mut Toggled, i: usize| s.hunk = Some(i));
        View::<Toggled>::rebuild(&next, &view(true, vec![false, true]), &mut w, &mut ctx);
        assert_eq!(
            w.hunks[0].reveal.value(),
            0.0,
            "a same-shape swap keeps the collapsed hunk collapsed"
        );
        assert_eq!(w.hunks.len(), 2);

        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let shrunk = file_diff::<Toggled>(file_diff_model(
            "src/agents/file_diff.rs",
            vec![diff_hunk(
                "@@ -1 +1 @@",
                vec![diff_line(FileDiffLineKind::Added, "x")],
            )],
        ))
        .open(true);
        View::<Toggled>::rebuild(&shrunk, &next, &mut w, &mut ctx);
        assert_eq!(w.hunks.len(), 1, "a shape change rebuilds the hunks");
        assert_eq!(w.focused, DiffTarget::File, "and resets the cursor");
    }

    // ---- Typeface: the code runs keep the mono stack -----------------------

    /// The explicit site: under the beUI theme, whose every role is Geist,
    /// every run of an open diff — file, counts, hunk headers, line numbers,
    /// markers and content — still paints in Geist Mono.
    #[test]
    fn every_run_stays_in_geist_mono_under_the_beui_theme() {
        use crate::text::typeface_probe::{Face, Probe, assert_all};

        let logic = |_: &mut ()| file_diff::<()>(model());
        let faces = Probe::new(logic, Size::new(600.0, 600.0), crate::theme()).frame();
        assert_all(
            "the diff's code runs",
            "under the beUI theme",
            &faces,
            Face::GeistMono,
        );
    }
}
