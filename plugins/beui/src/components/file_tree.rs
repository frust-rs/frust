//! Ports beUI's `file-tree` component.
//!
//! **Source:** `components/motion/file-tree.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | row `h-9 gap-2 rounded-lg pr-2`, `paddingLeft: 8 + depth*indent` | [`TREE_ROW_HEIGHT`], [`TREE_ROW_GAP`], [`TREE_ROW_PADDING_LEFT`], [`TREE_INDENT`] |
//! | `SharedLayoutBg` pill `rounded-xl bg-muted` + `SPRING_LAYOUT` | the selection pill's rect springs between rows |
//! | row `text-muted-foreground`, selected `bg-muted font-medium text-foreground` | the resolved row inks |
//! | branch `absolute w-px bg-border/70`, `left: 16 + (depth−1)*indent` | [`TREE_BRANCH_X`], [`TREE_BRANCH_ALPHA`] |
//! | chevron `animate={{ rotate: isOpen ? 90 : 0 }}` on `SPRING_SWAP` | the chevron's rotation follows the reveal |
//! | `ROW_ENTER` `{0.22s, EASE_OUT}` + `delay: min(position*0.025, 0.1)` | [`TREE_ROW_ENTER`], [`TREE_ROW_STAGGER`], [`TREE_ROW_STAGGER_CAP`] |
//! | row `initial={{ opacity: 0, y: −6 }}` | [`TREE_ROW_RISE`] |
//! | `aria-disabled` `opacity-0.42` | [`TREE_DISABLED_OPACITY`] |
//! | `ArrowUp`/`Down`/`Left`/`Right`/`Home`/`End`/`Enter`/`Space` | the same traversal, on the widget's own roving focus |
//!
//! # A leaf widget: every row is painted, none is a child view
//!
//! Upstream's rows carry `lucide` icons and arbitrary `ReactNode` names. This
//! port takes plain strings and paints its own chevron, folder and file marks,
//! which makes the whole tree one leaf widget with no `ChildPod`s at all. That
//! is what lets a row appear, disappear and slide during an expand without any
//! child reconciliation — the animation is arithmetic over a flattened row list,
//! not a mount/unmount.
//!
//! # The expand animation, and why `paint` asks for layout
//!
//! Expanding is not a per-row entrance played in place; the rows *below* the
//! subtree have to move out of its way, and the widget's own height changes with
//! them. One toggle therefore drives three things off a single `0 → 1` reveal:
//!
//! * the subtree's band is `reveal · (rows · TREE_ROW_HEIGHT)` tall and clipped
//!   to that, so the children wipe in from the fold rather than appearing whole;
//! * every row *after* the band is lifted by the band's unrevealed remainder, so
//!   the rows below slide;
//! * each row *inside* the band fades and rises on its own staggered slot.
//!
//! The reported height is computed from that reveal in `layout`, and the reveal
//! advances on a clock only `paint` has — so, exactly as
//! `frust_glyph::accordion` states, `paint` calls `PaintCtx::request_layout`
//! while the reveal runs, and once more on the frame it lands (which reports
//! "not running"). A bare `request_frame` would let the height freeze mid-reveal
//! on the intra-frame layout skip.
//!
//! A collapse is the same run backwards: the folder is kept in the flattened
//! list for the length of its own reveal so there is something to wipe *out*,
//! and dropped when it settles.
//!
//! **One toggle animates at a time.** Starting a second while one is in flight
//! lands the first immediately. Upstream has no such limit (React unmounts and
//! `AnimatePresence` overlaps freely), and overlapping band arithmetic on one
//! flattened list would need a band per fold; the case is a double-click on two
//! folders inside 300ms.
//!
//! # Degradations against the web original
//!
//! - **No open/closed folder icon swap.** Upstream cross-fades `Folder` and
//!   `FolderOpen` through an `AnimatePresence` `popLayout`. Here the chevron's
//!   rotation is the whole open/closed signal and the folder mark is drawn once
//!   — two vector marks morphing into each other is a component of its own.
//! - **No per-row `icon` override and no `className` hooks.** A row is a name
//!   and a kind; a catalog widget takes tokens, not class strings.
//! - **The branch guide does not draw itself in.** Upstream animates
//!   `scaleY: 0 → 1` on each guide (`BRANCH_DRAW`); here a guide is present
//!   whenever its row is, and the row's own fade carries the arrival. The guide
//!   is one hairline behind a fading row, and a second timeline for it would be
//!   invisible.
//! - **Focus is the widget's, not the row's.** Upstream gives every row
//!   `tabIndex` and moves DOM focus; the tree here is one focusable widget with
//!   an internal roving cursor, so `ArrowDown` moves the cursor and the
//!   framework's focus stays on the tree.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, EventCtx,
    EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerButton, PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget,
    erase_callback_arg,
    text::{FontWeight, TextContext, TextLayout, TextStyle},
};
use frust::{FrameTime, Theme};

use crate::motion::Ramp;
use crate::style;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};

/// One row's height, in logical px (`h-9`).
pub const TREE_ROW_HEIGHT: f64 = 36.0;

/// Each depth level's extra indent, in logical px (the `indent` prop's default).
pub const TREE_INDENT: f64 = 18.0;

/// A row's left padding at depth zero, in logical px (`paddingLeft: 8 + …`).
pub const TREE_ROW_PADDING_LEFT: f64 = 8.0;

/// A row's right padding, in logical px (`pr-2`).
pub const TREE_ROW_PADDING_RIGHT: f64 = 8.0;

/// The gap between a row's chevron, mark and name, in logical px (`gap-2`).
pub const TREE_ROW_GAP: f64 = 8.0;

/// The chevron's and the mark's box, in logical px (`size-4`).
pub const TREE_GLYPH_SIZE: f64 = 16.0;

/// A branch guide's x at depth `d`, in logical px (`left: 16 + (depth−1)*indent`).
pub fn tree_branch_x(depth: usize) -> f64 {
    16.0 + (depth.saturating_sub(1)) as f64 * TREE_INDENT
}

/// The branch guide's alpha (`bg-border/70`).
pub const TREE_BRANCH_ALPHA: f32 = 0.70;

/// A disabled row's opacity (`opacity: 0.42`).
pub const TREE_DISABLED_OPACITY: f32 = 0.42;

/// How far a revealing row rises into place, in logical px (`initial={{ y: −6 }}`).
pub const TREE_ROW_RISE: f64 = 6.0;

/// A revealing row's own ramp (`ROW_ENTER = { duration: 0.22, ease: EASE_OUT }`).
pub const TREE_ROW_ENTER: Ramp = Ramp::eased(Duration::from_millis(220), EASE_OUT);

/// How far apart consecutive revealing rows start (`delay: position * 0.025`).
pub const TREE_ROW_STAGGER: Duration = Duration::from_millis(25);

/// The cap on that delay (`Math.min(…, 0.1)`) — after four rows every further
/// row starts together.
pub const TREE_ROW_STAGGER_CAP: Duration = Duration::from_millis(100);

/// Whether a node is a file or a folder (upstream's `type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTreeKind {
    /// A leaf. Never expandable, never chevroned.
    File,
    /// A branch. Expandable whenever it has children.
    Folder,
}

/// One node of the declarative tree.
#[derive(Clone, Debug)]
pub struct FileTreeNode {
    value: String,
    name: String,
    kind: FileTreeKind,
    disabled: bool,
    children: Vec<FileTreeNode>,
}

/// Create a file leaf identified by `value` and shown as `name`.
pub fn file_tree_file(value: impl Into<String>, name: impl Into<String>) -> FileTreeNode {
    FileTreeNode {
        value: value.into(),
        name: name.into(),
        kind: FileTreeKind::File,
        disabled: false,
        children: Vec::new(),
    }
}

/// Create a folder identified by `value`, shown as `name`, holding `children`.
pub fn file_tree_folder(
    value: impl Into<String>,
    name: impl Into<String>,
    children: Vec<FileTreeNode>,
) -> FileTreeNode {
    FileTreeNode {
        value: value.into(),
        name: name.into(),
        kind: FileTreeKind::Folder,
        disabled: false,
        children,
    }
}

impl FileTreeNode {
    /// Make this node inert: dimmed, unpressable, and (for a folder)
    /// un-expandable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// This node's identity.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// This node's displayed name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether it is a file or a folder.
    pub fn kind(&self) -> FileTreeKind {
        self.kind
    }
}

/// A cached text run whose colour is applied at paint time.
struct Run {
    content: String,
    layout: Option<TextLayout>,
    shaped: Option<TextStyle>,
}

impl Run {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped: None,
        }
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped.as_ref() == Some(style)
        {
            return cached.size();
        }
        let laid = ctx
            .text_context::<TextContext>()
            .layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped = Some(style.clone());
        size
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for mut run in layout.to_scene_runs(origin) {
                run.brush = Brush::Solid(color);
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A view-held string callback, erased on build.
type OnValue<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI file tree. See the [module docs](self).
pub struct FileTreeView<State: 'static> {
    nodes: Vec<FileTreeNode>,
    selected: Option<String>,
    expanded: Vec<String>,
    on_select: Option<OnValue<State>>,
    on_toggle: Option<OnValue<State>>,
}

/// Create a file tree over `nodes` — **controlled**: nothing is selected and
/// nothing is expanded until [`selected`](FileTreeView::selected) and
/// [`expanded`](FileTreeView::expanded) say so, and the widget never writes
/// either itself.
pub fn file_tree<State: 'static>(nodes: Vec<FileTreeNode>) -> FileTreeView<State> {
    FileTreeView {
        nodes,
        selected: None,
        expanded: Vec::new(),
        on_select: None,
        on_toggle: None,
    }
}

impl<State: 'static> FileTreeView<State> {
    /// The value of the selected row, if any.
    pub fn selected(mut self, selected: impl Into<String>) -> Self {
        self.selected = Some(selected.into());
        self
    }

    /// The values of the expanded folders.
    pub fn expanded(mut self, expanded: Vec<String>) -> Self {
        self.expanded = expanded;
        self
    }

    /// Report a row the user chose.
    pub fn on_select<F: Fn(&mut State, String) + 'static>(mut self, on_select: F) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Report a folder the user asked to expand or collapse. The *set* is the
    /// app's to maintain; this reports the one folder whose state was toggled.
    pub fn on_toggle<F: Fn(&mut State, String) + 'static>(mut self, on_toggle: F) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }
}

/// The resolved tree palette.
struct TreeColors {
    /// The selection pill (`bg-muted`).
    pill: Color,
    /// A selected or hovered row's ink (`text-foreground`).
    active_ink: Color,
    /// An ordinary row's ink (`text-muted-foreground`).
    ink: Color,
    /// The branch guide, already at [`TREE_BRANCH_ALPHA`] (`bg-border/70`).
    branch: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> TreeColors {
    let (muted, foreground, muted_foreground, border) = match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.surface_container_highest,
                s.on_surface,
                s.on_surface_variant,
                s.outline_variant,
            )
        }
        None => {
            let p = crate::BEUI_LIGHT;
            (p.muted, p.foreground, p.muted_foreground, p.border)
        }
    };
    TreeColors {
        pill: muted,
        active_ink: foreground,
        ink: muted_foreground,
        branch: style::with_alpha(border, TREE_BRANCH_ALPHA),
    }
}

/// The row name style (`text-sm`; a selected row is `font-medium`).
fn name_style(theme: Option<&Theme>, selected: bool) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: if selected {
            FontWeight::MEDIUM
        } else {
            FontWeight::REGULAR
        },
        ..TextStyle::new(style::TEXT_SM as f32, Color::BLACK)
    }
}

/// Whether `p` carries a button that may begin a press.
fn presses(p: &PointerEvent) -> bool {
    p.button == PointerButton::Primary
}

/// Linear interpolation between two rects, `t` unclamped.
fn lerp_rect(from: Rect, to: Rect, t: f64) -> Rect {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x0, to.x0),
        lerp(from.y0, to.y0),
        lerp(from.x1, to.x1),
        lerp(from.y1, to.y1),
    )
}

/// One flattened, visible row.
struct Row {
    value: String,
    name: Run,
    kind: FileTreeKind,
    disabled: bool,
    depth: usize,
    /// Whether this folder has children to disclose at all.
    expandable: bool,
    /// The value of this row's parent folder, if it has one.
    parent: Option<String>,
}

/// Flatten `nodes` against the expanded set, depth-first, exactly as upstream's
/// `flattenItems` does.
fn flatten(
    nodes: &[FileTreeNode],
    expanded: &dyn Fn(&str) -> bool,
    depth: usize,
    parent: Option<&str>,
    out: &mut Vec<Row>,
) {
    for node in nodes {
        let expandable = node.kind == FileTreeKind::Folder && !node.children.is_empty();
        out.push(Row {
            value: node.value.clone(),
            name: Run::new(node.name.clone()),
            kind: node.kind,
            disabled: node.disabled,
            depth,
            expandable,
            parent: parent.map(str::to_owned),
        });
        if expandable && expanded(&node.value) {
            flatten(&node.children, expanded, depth + 1, Some(&node.value), out);
        }
    }
}

/// A fold in flight.
#[derive(Clone, Debug, PartialEq)]
struct Fold {
    /// The folder being opened or closed.
    folder: String,
    /// `true` while opening, `false` while closing.
    opening: bool,
    /// The first row of the affected subtree.
    first: usize,
    /// How many rows the subtree contributes.
    count: usize,
    started: Option<FrameTime>,
}

impl<State: 'static> View<State> for FileTreeView<State> {
    type Element = FileTreeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FileTreeWidget {
        let mut widget = FileTreeWidget {
            nodes: self.nodes.clone(),
            expanded: self.expanded.clone(),
            selected: self.selected.clone(),
            rows: Vec::new(),
            fold: None,
            reveal: 1.0,
            focused: None,
            hovered: None,
            captured: None,
            pill_from: None,
            pill_shown: None,
            pill_started: None,
            width: 0.0,
            on_select: self.on_select.as_ref().map(erase_callback_arg),
            on_toggle: self.on_toggle.as_ref().map(erase_callback_arg),
        };
        widget.refresh_rows();
        widget.focused = widget.rows.first().map(|row| row.value.clone());
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FileTreeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = self.on_select.as_ref().map(erase_callback_arg);
        element.on_toggle = self.on_toggle.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        if prev.selected != self.selected {
            element.pill_from = element.pill_shown.or_else(|| element.pill_target());
            element.pill_started = None;
            element.selected = self.selected.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let structure_changed = prev.nodes.len() != self.nodes.len()
            || prev
                .nodes
                .iter()
                .zip(self.nodes.iter())
                .any(|(a, b)| a.value != b.value || a.name != b.name);
        if structure_changed {
            element.nodes = self.nodes.clone();
            element.fold = None;
            element.reveal = 1.0;
            element.expanded = self.expanded.clone();
            element.refresh_rows();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if prev.expanded != self.expanded {
            element.stage_fold(&self.expanded);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }
}

/// The retained widget for a [`FileTreeView`].
pub struct FileTreeWidget {
    nodes: Vec<FileTreeNode>,
    /// The app-confirmed expanded set (source of truth, adopted on `rebuild`).
    expanded: Vec<String>,
    /// The app-confirmed selection.
    selected: Option<String>,
    /// The flattened visible rows, including a collapsing folder's children for
    /// the length of their own fold.
    rows: Vec<Row>,
    /// The fold in flight, if any.
    fold: Option<Fold>,
    /// How revealed the folding band is, `0` closed to `1` open. Written in
    /// `paint`, read by `layout`.
    reveal: f64,
    /// The row the roving cursor sits on, by value.
    focused: Option<String>,
    /// The latched hovered row, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The row a `Down` armed.
    captured: Option<usize>,
    /// The rect the selection pill's travel started from.
    pill_from: Option<Rect>,
    /// The rect the pill painted last frame — the retarget origin.
    pill_shown: Option<Rect>,
    /// The frame the current pill travel was first painted at.
    pill_started: Option<FrameTime>,
    /// The widest row layout resolved.
    width: f64,
    on_select: Option<frust::authoring::ErasedArgCallback<String>>,
    on_toggle: Option<frust::authoring::ErasedArgCallback<String>>,
}

impl FileTreeWidget {
    /// Re-flatten the visible rows against the expanded set plus, while a
    /// collapse is playing, the folder that is closing.
    fn refresh_rows(&mut self) {
        let expanded = self.expanded.clone();
        let holding = self
            .fold
            .as_ref()
            .filter(|fold| !fold.opening)
            .map(|fold| fold.folder.clone());
        let mut rows = Vec::new();
        flatten(
            &self.nodes,
            &|value: &str| expanded.iter().any(|v| v == value) || holding.as_deref() == Some(value),
            0,
            None,
            &mut rows,
        );
        self.rows = rows;
        self.hovered = None;
        self.captured = None;
    }

    /// Adopt a new expanded set and stage the fold it represents.
    ///
    /// A fold already in flight is landed first — one band animates at a time
    /// (see the [module docs](self)).
    fn stage_fold(&mut self, next: &[String]) {
        self.fold = None;
        self.reveal = 1.0;

        let opened = next.iter().find(|value| !self.expanded.contains(value));
        let closed = self
            .expanded
            .iter()
            .find(|value| !next.contains(value))
            .cloned();
        let staged = opened
            .cloned()
            .map(|value| (value, true))
            .or_else(|| closed.map(|value| (value, false)));

        self.expanded = next.to_vec();
        let Some((folder, opening)) = staged else {
            self.refresh_rows();
            return;
        };

        // Flatten with the folder open either way: an opening band has to exist
        // to wipe in, and a closing one has to exist to wipe out.
        self.fold = Some(Fold {
            folder: folder.clone(),
            opening,
            first: 0,
            count: 0,
            started: None,
        });
        self.refresh_rows();

        let Some(index) = self.rows.iter().position(|row| row.value == folder) else {
            self.fold = None;
            return;
        };
        let depth = self.rows[index].depth;
        let count = self.rows[index + 1..]
            .iter()
            .take_while(|row| row.depth > depth)
            .count();
        if count == 0 {
            self.fold = None;
            return;
        }
        self.reveal = if opening { 0.0 } else { 1.0 };
        self.fold = Some(Fold {
            folder,
            opening,
            first: index + 1,
            count,
            started: None,
        });
    }

    /// The folding band's full height.
    fn band_height(&self) -> f64 {
        self.fold
            .as_ref()
            .map_or(0.0, |fold| fold.count as f64 * TREE_ROW_HEIGHT)
    }

    /// How much of the band is currently hidden — what every row below it is
    /// lifted by.
    fn band_hidden(&self) -> f64 {
        self.band_height() * (1.0 - self.reveal)
    }

    /// Row `index`'s box, with the fold's displacement applied.
    fn row_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.rows.len() {
            return None;
        }
        let mut y = index as f64 * TREE_ROW_HEIGHT;
        if let Some(fold) = &self.fold
            && index >= fold.first + fold.count
        {
            y -= self.band_hidden();
        }
        Some(Rect::from_origin_size(
            Point::new(0.0, y),
            Size::new(self.width, TREE_ROW_HEIGHT),
        ))
    }

    /// Whether row `index` is inside the folding band.
    fn in_band(&self, index: usize) -> bool {
        self.fold
            .as_ref()
            .is_some_and(|fold| index >= fold.first && index < fold.first + fold.count)
    }

    /// Whether folder `value` reads as open right now.
    fn is_open(&self, value: &str) -> bool {
        self.expanded.iter().any(|v| v == value)
    }

    /// Whether row `index` can be interacted with.
    fn enabled(&self, index: usize) -> bool {
        self.rows.get(index).is_some_and(|row| !row.disabled)
    }

    /// The selection pill's resting rect.
    fn pill_target(&self) -> Option<Rect> {
        let selected = self.selected.as_deref()?;
        let index = self.rows.iter().position(|row| row.value == selected)?;
        self.row_rect(index)
    }

    /// The row under a widget-local `pos`, if any.
    fn hit_row(&self, pos: Point) -> Option<usize> {
        (0..self.rows.len()).find(|index| {
            self.row_rect(*index).is_some_and(|rect| {
                rect.contains(Point::new(rect.x0.max(pos.x.min(rect.x1 - 0.001)), pos.y))
            })
        })
    }

    /// The index of the focused row, if it is still visible.
    fn focused_index(&self) -> Option<usize> {
        let focused = self.focused.as_deref()?;
        self.rows.iter().position(|row| row.value == focused)
    }

    /// How far into the reveal row `index` is, in `[0, 1]`, at `elapsed`.
    ///
    /// The per-row delay is `min(position · 25ms, 100ms)`, computed inline
    /// because [`crate::motion::Stagger`] has no cap on its accumulated offset
    /// and upstream's does.
    fn row_reveal(&self, index: usize, elapsed: Duration, opening: bool) -> f64 {
        let Some(fold) = &self.fold else {
            return 1.0;
        };
        let position = index.saturating_sub(fold.first);
        let delay = (TREE_ROW_STAGGER * position as u32).min(TREE_ROW_STAGGER_CAP);
        let progress = TREE_ROW_ENTER.progress_clamped(elapsed.saturating_sub(delay));
        if opening { progress } else { 1.0 - progress }
    }

    /// Advance the fold to `now`, returning its elapsed time and whether it is
    /// still running.
    fn advance_fold(&mut self, now: FrameTime, reduce_motion: bool) -> (Duration, bool) {
        let Some(fold) = self.fold.as_mut() else {
            return (Duration::ZERO, false);
        };
        if reduce_motion {
            self.fold = None;
            self.reveal = 1.0;
            self.refresh_rows();
            return (Duration::ZERO, false);
        }
        let opening = fold.opening;
        let count = fold.count;
        let started = *fold.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        // The band is done when its last (capped) slot has finished.
        let total = TREE_ROW_ENTER.settle()
            + (TREE_ROW_STAGGER * count.saturating_sub(1) as u32).min(TREE_ROW_STAGGER_CAP);
        if elapsed >= total {
            self.fold = None;
            self.reveal = 1.0;
            self.refresh_rows();
            return (elapsed, false);
        }
        let band = TREE_ROW_ENTER.progress_clamped(elapsed);
        self.reveal = if opening { band } else { 1.0 - band };
        (elapsed, true)
    }

    /// Advance the selection pill to `now`.
    fn advance_pill(&mut self, now: FrameTime, reduce_motion: bool) -> (Option<Rect>, bool) {
        let Some(target) = self.pill_target() else {
            self.pill_shown = None;
            self.pill_from = None;
            return (None, false);
        };
        let Some(from) = self.pill_from.filter(|_| !reduce_motion) else {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        };
        let ramp = Ramp::spring(SPRING_LAYOUT);
        let started = *self.pill_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        }
        let shown = lerp_rect(from, target, ramp.progress(elapsed));
        self.pill_shown = Some(shown);
        (Some(shown), true)
    }

    /// Report a chosen row, and toggle it when it is an expandable folder —
    /// upstream's click handler does both.
    fn activate(&mut self, ctx: &mut EventCtx, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if row.disabled {
            return;
        }
        let value = row.value.clone();
        let expandable = row.expandable;
        self.focused = Some(value.clone());
        if let Some(on_select) = &mut self.on_select {
            on_select(ctx, value.clone());
        }
        if expandable && let Some(on_toggle) = &mut self.on_toggle {
            on_toggle(ctx, value);
        }
    }

    /// Ask the app to toggle folder at `index`.
    fn request_toggle(&mut self, ctx: &mut EventCtx, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if row.disabled || !row.expandable {
            return;
        }
        let value = row.value.clone();
        if let Some(on_toggle) = &mut self.on_toggle {
            on_toggle(ctx, value);
        }
    }

    /// Move the roving cursor to row `index`.
    fn focus_row(&mut self, index: usize) {
        if let Some(row) = self.rows.get(index) {
            self.focused = Some(row.value.clone());
        }
    }
}

/// Paint a chevron pointing right, rotated `angle` radians about `centre`.
fn draw_chevron(scene: &mut dyn PaintScene, centre: Point, angle: f64, color: Color) {
    let arm = TREE_GLYPH_SIZE * 0.22;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm * 0.6, -arm));
    path.line_to(Point::new(arm * 0.6, 0.0));
    path.line_to(Point::new(-arm * 0.6, arm));
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    scene.stroke_path(Point::ZERO, &path, 1.5, &Brush::Solid(color));
    scene.pop_transform();
}

/// Paint a folder or file mark inside a `TREE_GLYPH_SIZE` box at `origin`.
fn draw_mark(scene: &mut dyn PaintScene, origin: Point, kind: FileTreeKind, color: Color) {
    let s = TREE_GLYPH_SIZE;
    let mut path = BezPath::new();
    match kind {
        FileTreeKind::Folder => {
            // A body with a tab on its top-left corner.
            path.move_to(Point::new(s * 0.1, s * 0.3));
            path.line_to(Point::new(s * 0.42, s * 0.3));
            path.line_to(Point::new(s * 0.52, s * 0.42));
            path.line_to(Point::new(s * 0.9, s * 0.42));
            path.line_to(Point::new(s * 0.9, s * 0.82));
            path.line_to(Point::new(s * 0.1, s * 0.82));
            path.close_path();
        }
        FileTreeKind::File => {
            // A sheet with a clipped corner.
            path.move_to(Point::new(s * 0.22, s * 0.16));
            path.line_to(Point::new(s * 0.6, s * 0.16));
            path.line_to(Point::new(s * 0.8, s * 0.36));
            path.line_to(Point::new(s * 0.8, s * 0.84));
            path.line_to(Point::new(s * 0.22, s * 0.84));
            path.close_path();
        }
    }
    scene.stroke_path(origin, &path, 1.25, &Brush::Solid(color));
}

impl Widget for FileTreeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let selected = self.selected.clone();
        let plain = name_style(theme, false);
        let medium = name_style(theme, true);
        let mut content: f64 = 0.0;
        for row in &mut self.rows {
            let is_selected = selected.as_deref() == Some(row.value.as_str());
            let style = if is_selected { &medium } else { &plain };
            let name = row.name.layout(ctx, style);
            let lead = TREE_ROW_PADDING_LEFT
                + row.depth as f64 * TREE_INDENT
                + TREE_GLYPH_SIZE
                + TREE_ROW_GAP
                + TREE_GLYPH_SIZE
                + TREE_ROW_GAP;
            content = content.max(lead + name.width + TREE_ROW_PADDING_RIGHT);
        }
        self.width = content.clamp(bc.min().width, bc.max().width);

        let height = self.rows.len() as f64 * TREE_ROW_HEIGHT - self.band_hidden();
        bc.constrain(Size::new(self.width, height.max(0.0)))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();

        let reveal_before = self.reveal;
        let opening = self.fold.as_ref().is_some_and(|fold| fold.opening);
        let (elapsed, folding) = self.advance_fold(ctx.frame_time(), reduce_motion);
        let (pill, pill_running) = self.advance_pill(ctx.frame_time(), reduce_motion);

        // The band is clipped to its revealed height so folding children wipe in
        // and out of the fold rather than appearing whole.
        let band_clip = self.fold.as_ref().map(|fold| {
            let top = fold.first as f64 * TREE_ROW_HEIGHT;
            (
                Point::new(origin.x, origin.y + top),
                Size::new(self.width, (self.band_height() * self.reveal).max(0.0)),
            )
        });

        if let Some(rect) = pill {
            scene.fill_rounded_rect(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                Size::new(rect.width().max(0.0), rect.height()),
                style::RADIUS_XL,
                colors.pill,
            );
        }

        for index in 0..self.rows.len() {
            let Some(rect) = self.row_rect(index) else {
                continue;
            };
            let banded = self.in_band(index);
            let reveal = if banded && folding {
                self.row_reveal(index, elapsed, opening)
            } else {
                1.0
            };
            if reveal <= 0.0 {
                continue;
            }
            if banded && let Some((clip_origin, clip_size)) = band_clip {
                scene.push_clip(clip_origin, clip_size);
            }

            let row = &self.rows[index];
            let selected = self.selected.as_deref() == Some(row.value.as_str());
            let ink = if selected || (self.hovered == Some(index) && !row.disabled) {
                colors.active_ink
            } else {
                colors.ink
            };
            let ink = style::disabled_tint(ink, row.disabled, TREE_DISABLED_OPACITY);
            let ink = style::scale_alpha(ink, reveal as f32);
            // A revealing row rises the last few px into place.
            let y = origin.y + rect.y0 - TREE_ROW_RISE * (1.0 - reveal);

            // The branch guide for every level this row hangs under.
            if row.depth > 0 {
                scene.fill_rect(
                    Point::new(origin.x + tree_branch_x(row.depth), y),
                    Size::new(style::BORDER_WIDTH, TREE_ROW_HEIGHT),
                    style::scale_alpha(colors.branch, reveal as f32),
                );
            }

            let lead = TREE_ROW_PADDING_LEFT + row.depth as f64 * TREE_INDENT;
            if row.expandable {
                // The chevron: right when closed, down when open, carried by the
                // fold's own reveal while one is playing.
                let open_amount = if self.fold.as_ref().is_some_and(|f| f.folder == row.value) {
                    self.reveal
                } else if self.is_open(&row.value) {
                    1.0
                } else {
                    0.0
                };
                draw_chevron(
                    scene,
                    Point::new(
                        origin.x + lead + TREE_GLYPH_SIZE / 2.0,
                        y + TREE_ROW_HEIGHT / 2.0,
                    ),
                    open_amount * std::f64::consts::FRAC_PI_2,
                    ink,
                );
            }

            let mark_x = origin.x + lead + TREE_GLYPH_SIZE + TREE_ROW_GAP;
            draw_mark(
                scene,
                Point::new(mark_x, y + (TREE_ROW_HEIGHT - TREE_GLYPH_SIZE) / 2.0),
                row.kind,
                ink,
            );

            let name = row.name.size();
            row.name.paint(
                Point::new(
                    mark_x + TREE_GLYPH_SIZE + TREE_ROW_GAP,
                    y + (TREE_ROW_HEIGHT - name.height) / 2.0,
                ),
                ink,
                scene,
            );

            if banded && band_clip.is_some() {
                scene.pop_clip();
            }
        }

        // The reported height is computed in `layout` from `reveal`, so a bare
        // frame request would let the tree freeze mid-fold on the intra-frame
        // layout skip. Ask for layout while folding, and once more on the frame
        // the value landed (which reports "not folding").
        if folding {
            ctx.request_layout();
        }
        if self.reveal != reveal_before {
            ctx.request_layout();
        }
        if pill_running {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => self.key_event(ctx, key),
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_row(p.position).filter(|i| self.enabled(*i)) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(index);
                    self.focus_row(index);
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
                    let over = self.hit_row(p.position).filter(|i| self.enabled(*i));
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
                    if self.hit_row(p.position) == Some(armed) {
                        self.activate(ctx, armed);
                    }
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Tree,
            |_| {},
            |ctx| {
                for row in &self.rows {
                    ctx.push_node(Role::TreeItem, |node| {
                        node.set_label(row.name.content.as_str());
                        node.set_selected(self.selected.as_deref() == Some(row.value.as_str()));
                        if row.expandable {
                            node.set_expanded(self.is_open(&row.value));
                        }
                        if row.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }
}

impl FileTreeWidget {
    /// The keyboard traversal, transcribed from upstream's `handleKeyDown`.
    fn key_event(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> EventResult {
        let Some(index) =
            self.focused_index()
                .or(if self.rows.is_empty() { None } else { Some(0) })
        else {
            return EventResult::Ignored;
        };
        let last = self.rows.len().saturating_sub(1);
        let is_folder = self.rows[index].expandable;
        let is_open = is_folder && self.is_open(&self.rows[index].value);

        match &key.key {
            Key::Named(NamedKey::ArrowDown) if index < last => {
                self.focus_row(index + 1);
                ctx.request_redraw();
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowUp) if index > 0 => {
                self.focus_row(index - 1);
                ctx.request_redraw();
                EventResult::Handled
            }
            Key::Named(NamedKey::Home) => {
                self.focus_row(0);
                ctx.request_redraw();
                EventResult::Handled
            }
            Key::Named(NamedKey::End) => {
                self.focus_row(last);
                ctx.request_redraw();
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowRight) if is_folder => {
                if is_open {
                    // Already open: descend into the first child, which is the
                    // next row by construction.
                    if self
                        .rows
                        .get(index + 1)
                        .and_then(|row| row.parent.as_deref())
                        == Some(self.rows[index].value.as_str())
                    {
                        self.focus_row(index + 1);
                        ctx.request_redraw();
                    }
                } else {
                    self.request_toggle(ctx, index);
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if is_folder && is_open {
                    self.request_toggle(ctx, index);
                } else if let Some(parent) = self.rows[index].parent.clone() {
                    self.focused = Some(parent);
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::Enter) => {
                self.activate(ctx, index);
                EventResult::Handled
            }
            Key::Character(text) if text == " " => {
                self.activate(ctx, index);
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::Modifiers;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        inks: Vec<Color>,
        clips: Vec<(Point, Size)>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_transform(&mut self, _t: Affine) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Picked {
        selected: Vec<String>,
        toggled: Vec<String>,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// `src/` (`app.rs`, `ui/` with two files), `README.md`, and a disabled
    /// `target/`.
    fn nodes() -> Vec<FileTreeNode> {
        vec![
            file_tree_folder(
                "src",
                "src",
                vec![
                    file_tree_file("app", "app.rs"),
                    file_tree_folder(
                        "ui",
                        "ui",
                        vec![
                            file_tree_file("button", "button.rs"),
                            file_tree_file("panel", "panel.rs"),
                        ],
                    ),
                ],
            ),
            file_tree_file("readme", "README.md"),
            file_tree_file("target", "target").disabled(true),
        ]
    }

    fn view(selected: Option<&str>, expanded: &[&str]) -> FileTreeView<Picked> {
        let mut v = file_tree::<Picked>(nodes())
            .expanded(expanded.iter().map(|s| (*s).to_owned()).collect())
            .on_select(|s: &mut Picked, v: String| s.selected.push(v))
            .on_toggle(|s: &mut Picked, v: String| s.toggled.push(v));
        if let Some(selected) = selected {
            v = v.selected(selected);
        }
        v
    }

    fn build(selected: Option<&str>, expanded: &[&str]) -> FileTreeWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(selected, expanded), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut FileTreeWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 800.0)),
        )
    }

    fn laid_out(selected: Option<&str>, expanded: &[&str]) -> (FileTreeWidget, Size) {
        let mut w = build(selected, expanded);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut FileTreeWidget,
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

    fn rebuild(w: &mut FileTreeWidget, from: (Option<&str>, &[&str]), to: (Option<&str>, &[&str])) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(to.0, to.1), &view(from.0, from.1), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn key_event(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(w: &mut FileTreeWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn values(w: &FileTreeWidget) -> Vec<&str> {
        w.rows.iter().map(|row| row.value.as_str()).collect()
    }

    /// Flattening follows the expanded set depth-first, and a collapsed folder
    /// contributes nothing but itself.
    #[test]
    fn the_expanded_set_decides_which_rows_exist() {
        let (closed, _) = laid_out(None, &[]);
        assert_eq!(values(&closed), vec!["src", "readme", "target"]);

        let (one, _) = laid_out(None, &["src"]);
        assert_eq!(values(&one), vec!["src", "app", "ui", "readme", "target"]);

        let (both, _) = laid_out(None, &["src", "ui"]);
        assert_eq!(
            values(&both),
            vec!["src", "app", "ui", "button", "panel", "readme", "target"]
        );
        assert_eq!(both.rows[3].depth, 2, "a nested child indents twice");
        assert_eq!(both.rows[3].parent.as_deref(), Some("ui"));
    }

    /// Expanding wipes the band in: the tree's height grows from the closed to
    /// the open one, the band is clipped to its revealed height the whole way,
    /// and every frame asks for **layout** rather than only a repaint.
    #[test]
    fn expanding_reveals_the_band_and_drives_layout() {
        let (mut w, size) = laid_out(None, &[]);
        let closed_height = size.height;
        paint_at(&mut w, size, None, 0.0);

        rebuild(&mut w, (None, &[]), (None, &["src"]));
        assert_eq!(w.reveal, 0.0, "the band starts closed");
        assert_eq!(values(&w), vec!["src", "app", "ui", "readme", "target"]);

        // The first frame latches the fold's start, so the band is still fully
        // folded and none of its rows is painted at all.
        let (first, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a folding tree must ask for relayout");
        assert!(first.clips.is_empty(), "nothing of the band is painted yet");

        // A frame later the band exists, clipped to less than its full height.
        let (rec, _, _) = paint_at(&mut w, size, None, 180.0);
        assert_eq!(rec.clips.len(), 2, "each banded row is clipped to the band");
        let clipped = rec.clips[0].1.height;
        assert!(
            clipped > 0.0 && clipped < 2.0 * TREE_ROW_HEIGHT,
            "the band is not partway open: {clipped}"
        );

        let mut previous = layout(&mut w).height;
        for step in 5..=12 {
            paint_at(&mut w, size, None, 100.0 + step as f64 * 20.0);
            let height = layout(&mut w).height;
            assert!(height >= previous, "the tree shrank mid-expand: {height}");
            previous = height;
        }

        paint_at(&mut w, size, None, 1_000.0);
        assert!(w.fold.is_none(), "the fold landed");
        assert_eq!(layout(&mut w).height, closed_height + 2.0 * TREE_ROW_HEIGHT);
    }

    /// Collapsing keeps the children flattened for the length of their own wipe,
    /// then drops them.
    #[test]
    fn collapsing_holds_the_children_until_the_wipe_finishes() {
        let (mut w, size) = laid_out(None, &["src"]);
        paint_at(&mut w, size, None, 0.0);
        rebuild(&mut w, (None, &["src"]), (None, &[]));

        assert_eq!(w.reveal, 1.0, "a closing band starts fully shown");
        assert_eq!(
            values(&w),
            vec!["src", "app", "ui", "readme", "target"],
            "the children are held for the wipe"
        );
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 160.0);
        assert!(w.reveal < 1.0 && w.reveal > 0.0, "mid-wipe: {}", w.reveal);

        paint_at(&mut w, size, None, 1_000.0);
        assert_eq!(values(&w), vec!["src", "readme", "target"]);
    }

    /// The band's rows arrive on a staggered, capped delay — upstream's
    /// `min(position * 0.025, 0.1)`.
    #[test]
    fn the_band_rows_arrive_on_a_capped_stagger() {
        let (mut w, _) = laid_out(None, &[]);
        rebuild(&mut w, (None, &[]), (None, &["src", "ui"]));
        let fold = w.fold.clone().expect("a fold is staged");
        assert_eq!(fold.first, 1);
        assert_eq!(fold.count, 4, "app, ui, button, panel");

        let at = |ms: u64| {
            (0..4)
                .map(|i| w.row_reveal(fold.first + i, Duration::from_millis(ms), true))
                .collect::<Vec<_>>()
        };
        let mid = at(60);
        assert!(mid[0] > mid[1] && mid[1] > mid[2], "not a cascade: {mid:?}");

        // The fifth slot onward shares the cap, so two rows past it are level.
        let capped = TREE_ROW_STAGGER * 5;
        assert!(capped > TREE_ROW_STAGGER_CAP);
        assert_eq!(
            w.row_reveal(fold.first + 4, Duration::from_millis(160), true),
            w.row_reveal(fold.first + 9, Duration::from_millis(160), true)
        );
    }

    /// A fold preserves the selection: the pill still targets the selected row
    /// after the tree grows around it, and it springs when the app moves it.
    #[test]
    fn a_fold_preserves_the_selection_and_the_pill_springs() {
        let (mut w, size) = laid_out(Some("readme"), &[]);
        paint_at(&mut w, size, None, 0.0);
        let before = w.pill_shown.expect("the pill rests on the selected row");
        assert_eq!(before, w.row_rect(1).unwrap());

        rebuild(&mut w, (Some("readme"), &[]), (Some("readme"), &["src"]));
        // Two frames: the first latches the fold's start, the second is past its
        // settle time.
        paint_at(&mut w, size, None, 1_000.0);
        paint_at(&mut w, size, None, 2_000.0);
        assert_eq!(w.selected.as_deref(), Some("readme"));
        let after = w.pill_shown.unwrap();
        assert_eq!(after, w.row_rect(3).unwrap(), "still on README.md");
        assert!(after.y0 > before.y0, "which the expand pushed down");

        // And a selection change springs rather than jumping.
        rebuild(&mut w, (Some("readme"), &["src"]), (Some("app"), &["src"]));
        let (_, needs_frame, _) = paint_at(&mut w, size, None, 2_100.0);
        assert!(needs_frame, "a travelling pill owes frames");
        paint_at(&mut w, size, None, 2_140.0);
        let mid = w.pill_shown.unwrap();
        assert!(
            mid.y0 < after.y0 && mid.y0 > w.row_rect(1).unwrap().y0,
            "mid-flight {mid:?}"
        );
        paint_at(&mut w, size, None, 6_000.0);
        assert_eq!(w.pill_shown, Some(w.row_rect(1).unwrap()));
    }

    /// `reduce_motion` lands the fold on the frame it is asked for.
    #[test]
    fn reduce_motion_lands_the_fold_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(None, &[]);
        paint_at(&mut w, size, Some(&theme), 0.0);
        rebuild(&mut w, (None, &[]), (None, &["src"]));
        paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(w.fold.is_none());
        assert_eq!(w.reveal, 1.0);
        assert_eq!(layout(&mut w).height, 5.0 * TREE_ROW_HEIGHT);
    }

    /// A press on a file reports a selection; a press on a folder reports the
    /// selection **and** the toggle, which is upstream's own click handler.
    #[test]
    fn a_press_selects_and_a_folder_press_also_toggles() {
        let (mut w, size) = laid_out(None, &[]);
        let mut state = Picked::default();

        let file = w.row_rect(1).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, file), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, file), &mut state);
        assert_eq!(state.selected, vec!["readme".to_owned()]);
        assert!(state.toggled.is_empty());

        let folder = w.row_rect(0).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, folder),
            &mut state,
        );
        dispatch(&mut w, size, &pointer(PointerPhase::Up, folder), &mut state);
        assert_eq!(state.selected, vec!["readme".to_owned(), "src".to_owned()]);
        assert_eq!(state.toggled, vec!["src".to_owned()]);
    }

    /// A disabled row is inert to the pointer.
    #[test]
    fn a_disabled_row_reports_nothing() {
        let (mut w, size) = laid_out(None, &[]);
        let mut state = Picked::default();
        let at = w.row_rect(2).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert!(state.selected.is_empty() && state.toggled.is_empty());
    }

    /// The traversal, transcribed: up/down walk the visible rows, Home/End jump,
    /// Right opens a closed folder then descends, Left closes an open one then
    /// climbs to the parent.
    #[test]
    fn the_arrow_keys_walk_the_visible_rows() {
        let (mut w, size) = laid_out(None, &["src"]);
        let mut state = Picked::default();
        assert_eq!(w.focused.as_deref(), Some("src"));

        dispatch(&mut w, size, &key_event(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused.as_deref(), Some("app"));
        dispatch(&mut w, size, &key_event(NamedKey::ArrowUp), &mut state);
        assert_eq!(w.focused.as_deref(), Some("src"));
        dispatch(&mut w, size, &key_event(NamedKey::End), &mut state);
        assert_eq!(w.focused.as_deref(), Some("target"));
        dispatch(&mut w, size, &key_event(NamedKey::Home), &mut state);
        assert_eq!(w.focused.as_deref(), Some("src"));

        // `src` is already open, so Right descends into its first child.
        dispatch(&mut w, size, &key_event(NamedKey::ArrowRight), &mut state);
        assert_eq!(w.focused.as_deref(), Some("app"));
        assert!(state.toggled.is_empty(), "descending toggles nothing");

        // From a child, Left climbs to the parent...
        dispatch(&mut w, size, &key_event(NamedKey::ArrowLeft), &mut state);
        assert_eq!(w.focused.as_deref(), Some("src"));
        // ...and from the open parent it asks to close it.
        dispatch(&mut w, size, &key_event(NamedKey::ArrowLeft), &mut state);
        assert_eq!(state.toggled, vec!["src".to_owned()]);
    }

    /// Right on a *closed* folder asks to open it rather than moving.
    #[test]
    fn right_opens_a_closed_folder() {
        let (mut w, size) = laid_out(None, &[]);
        let mut state = Picked::default();
        dispatch(&mut w, size, &key_event(NamedKey::ArrowRight), &mut state);
        assert_eq!(state.toggled, vec!["src".to_owned()]);
        assert_eq!(w.focused.as_deref(), Some("src"), "focus does not move");
    }

    /// `Enter` activates the focused row exactly as a press does.
    #[test]
    fn enter_activates_the_focused_row() {
        let (mut w, size) = laid_out(None, &[]);
        let mut state = Picked::default();
        dispatch(&mut w, size, &key_event(NamedKey::ArrowDown), &mut state);
        dispatch(&mut w, size, &key_event(NamedKey::Enter), &mut state);
        assert_eq!(state.selected, vec!["readme".to_owned()]);
    }

    /// A nested row draws its branch guide at upstream's own x.
    #[test]
    fn a_nested_row_draws_its_branch_guide() {
        let (mut w, size) = laid_out(None, &["src", "ui"]);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert_eq!(tree_branch_x(1), 16.0);
        assert_eq!(tree_branch_x(2), 16.0 + TREE_INDENT);
        let guides: Vec<_> = rec
            .rects
            .iter()
            .filter(|(_, s, _)| s.width == style::BORDER_WIDTH && s.height == TREE_ROW_HEIGHT)
            .collect();
        // One guide per row below the root: app, ui, button, panel.
        assert_eq!(guides.len(), 4, "wrong guide count: {guides:?}");
        assert!(
            guides.iter().any(|(o, _, _)| o.x == tree_branch_x(2)),
            "no depth-2 guide: {guides:?}"
        );
    }
}
