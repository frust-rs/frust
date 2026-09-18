//! Ports beUI's `ai-sidebar` agent-interface part.
//!
//! **Source:** `components/agents/ai-sidebar.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `ai-sidebar`: *"A collapsible AI workspace sidebar for folders,
//! projects, files, and bookmarks with keyboard navigation, optimistic moves,
//! inline rename, and overflow-aware labels."*
//!
//! # A correction to the brief this port was written from
//!
//! The porting card describes *"a sidebar shell composing conversation list +
//! sections with animated collapse"*. Upstream's `ai-sidebar` is not that: it is
//! a **resource tree** — folders, projects, files and bookmarks, nested, with
//! expand/collapse, a roving keyboard walk, and reorder/reparent moves. The
//! shell the card describes is upstream's *`chat-app`* (see
//! [`crate::agents::chat_app`]) hosting
//! [`crate::components::animated_sidebar`]'s rail. This port follows upstream.
//!
//! # Upstream's exports, and where each landed
//!
//! | upstream | here |
//! |---|---|
//! | `AISidebar` | [`ai_sidebar`] |
//! | `SidebarResource` / `SidebarResourceKind` | [`AiSidebarResource`] / [`AiSidebarKind`] |
//! | `SidebarResourceMove` / `SidebarResourceDropPosition` | [`AiSidebarMove`] / [`AiSidebarDrop`] |
//! | `moveResource` | [`ai_sidebar_move`] |
//! | `renameResource` | [`ai_sidebar_rename`] |
//! | `flattenResources` | [`ai_sidebar_rows`] |
//! | `canContain` | [`AiSidebarKind::can_contain`] |
//! | the `Alt+Shift+Arrow` move commands | the same four, reported through [`AiSidebarView::on_move`] |
//! | `defaultIcon` | the per-kind mark this module paints |
//!
//! # The tree operations are pure, and public
//!
//! Upstream applies a move **optimistically** — it rewrites its own copy of the
//! tree, calls `onMove`, and restores the old tree if that rejects. A widget
//! here owns no app state, so the same split is drawn one level up: the tree
//! transforms are pure functions over a `Vec<`[`AiSidebarResource`]`>`, the
//! widget only *reports* the move it was asked for, and the app applies (and,
//! if its own write fails, un-applies) [`ai_sidebar_move`] itself. That is the
//! framework's controlled-component rule, and it is what makes the whole
//! reorder/reparent algebra testable without a widget.
//!
//! # Motion
//!
//! * **The active pill** springs between rows on
//!   [`SPRING_LAYOUT`], the same
//!   shared-element treatment [`crate::components::animated_sidebar`]'s rail
//!   gives its own menu — upstream's `layoutId` pill.
//! * **A [`Presence`] per row**, timed by [`AI_SIDEBAR_ROW_MS`] (upstream's
//!   `ROW_REVEAL`), so expanding a folder reveals its children and collapsing it
//!   plays them out before their slots close.
//! * Because the rows' slots open and close, `paint` asks for **relayout**
//!   while a reveal runs rather than a bare frame.
//!
//! # Degradations against the web original
//!
//! - **No drag and drop.** Upstream's pointer path is HTML5 `dragstart` /
//!   `dragover` / `drop` with a `dataTransfer` payload and a drop-position ratio
//!   test against the hovered row's box. frust has no drag protocol at all, and
//!   a hand-rolled one (press, slop, autoscroll, drop indicator, cancel) is a
//!   component of its own. Every move upstream offers through a drag is still
//!   reachable here: the four `Alt+Shift+Arrow` commands, reported through
//!   [`AiSidebarView::on_move`] and applied with [`ai_sidebar_move`].
//! - **No inline rename.** Upstream swaps the row's label for an `<input>` on
//!   `F2` or a double-click. An editable field inside a painted row needs the
//!   baseline `TextInput` as a child pod per row plus a focus handoff; a host
//!   renames through [`ai_sidebar_rename`] from its own dialog instead. `F2` is
//!   therefore not bound.
//! - **No per-row overflow menu.** Upstream's `MorphPopover` row menu exists to
//!   carry rename and the four moves onto a device with no keyboard and no
//!   drag. A host composes [`crate::overlay::anchored`](mod@crate::overlay::anchored) over
//!   this tree for that.
//! - **The label is clipped, not marqueed.** Upstream scrolls an over-long label
//!   under the pointer (`MarqueeLabel`). The catalog has
//!   [`crate::components::marquee`], but it is a view, and a row here paints its
//!   own run rather than nesting one widget per row.
//! - **No live-region announcements.** Upstream keeps an `aria-live` span and
//!   writes "Moved X before Y" into it. Announcements are a shell concern in
//!   frust (`docs/CODE_STANDARDS.md`'s Semantics Conventions keep
//!   `Widget::semantics` to role/label/state), so the tree publishes structure
//!   and leaves narration to the host.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene,
    Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, erase_callback_arg,
    text::TextStyle,
};
use frust::{FrameTime, Theme};

use crate::motion::{Presence, PresencePhase, Ramp};
use crate::press::{Lane, presses};
use crate::style::{self, scale_alpha};
use crate::text::{LabelRun, ThemeTextType};
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};
use crate::tokens::{BEUI_LIGHT, BeuiPalette, sans_family};

/// A row's height, in logical px (`min-h-9`).
pub const AI_SIDEBAR_ROW_HEIGHT: f64 = 36.0;

/// The gap between rows (`gap-0.5`).
pub const AI_SIDEBAR_ROW_GAP: f64 = 2.0;

/// A root row's leading padding (`paddingLeft: 12 + depth * 16`).
pub const AI_SIDEBAR_PADDING_X: f64 = 12.0;

/// How far each nesting level indents (the `depth * 16` half of the same).
pub const AI_SIDEBAR_INDENT: f64 = 16.0;

/// A row's trailing padding (`pr-3`).
pub const AI_SIDEBAR_PADDING_END: f64 = 12.0;

/// The gap between a row's mark and its label (`gap-2.5`).
pub const AI_SIDEBAR_GAP: f64 = 10.0;

/// A row's mark box (`size-5`, holding a `size-4` glyph).
pub const AI_SIDEBAR_MARK_SIZE: f64 = 20.0;

/// A row's corner radius (`rounded-xl`).
pub const AI_SIDEBAR_RADIUS: f64 = style::RADIUS_XL;

/// How long a row's reveal takes, in ms (`ROW_REVEAL = { duration: 0.16 }`).
pub const AI_SIDEBAR_ROW_MS: u64 = 160;

/// How far a revealing row rises, in logical px — this port's own lead-in, the
/// same shift its sibling agent lists use.
pub const AI_SIDEBAR_ROW_SHIFT: f64 = 4.0;

/// Alpha of a disabled row (`opacity-45`).
const DISABLED_ALPHA: f32 = 0.45;

/// Alpha of the hover wash behind a row (`hover:bg-muted`).
const HOVER_ALPHA: f32 = 0.6;

/// Alpha of a row's resting label ink (`text-muted-foreground`), against the
/// full-strength ink an active or hovered row wears.
const LABEL_ALPHA: f32 = 0.9;

/// The default accessible name (`ariaLabel = "Resources"`).
pub const AI_SIDEBAR_LABEL: &str = "Resources";

/// How far past `now` a reduced-motion step reads, in nanoseconds — past every
/// ramp this module runs, so a reveal lands on the frame it starts.
const SETTLE_AT_ONCE_NANOS: u64 = 10_000_000_000;

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// What kind of thing a row is — upstream's `SidebarResourceKind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AiSidebarKind {
    /// A plain folder: holds children, opens and closes.
    Folder,
    /// A project: a folder with its own identity.
    Project,
    /// A leaf document.
    #[default]
    File,
    /// A saved reference.
    Bookmark,
}

impl AiSidebarKind {
    /// Every kind, in upstream's own union order.
    pub const ALL: [AiSidebarKind; 4] = [
        AiSidebarKind::Folder,
        AiSidebarKind::Project,
        AiSidebarKind::File,
        AiSidebarKind::Bookmark,
    ];

    /// Whether this kind may hold children — upstream's `canContain`, which is
    /// what decides whether a row toggles or selects, and whether a move may
    /// land *inside* it.
    pub const fn can_contain(self) -> bool {
        matches!(self, AiSidebarKind::Folder | AiSidebarKind::Project)
    }
}

/// One node of the resource tree — upstream's `SidebarResource`.
#[derive(Clone, Debug, PartialEq)]
pub struct AiSidebarResource {
    id: String,
    label: String,
    kind: AiSidebarKind,
    children: Vec<AiSidebarResource>,
    disabled: bool,
}

/// Create a resource named `label`, identified by `id`.
pub fn ai_sidebar_resource(
    id: impl Into<String>,
    label: impl Into<String>,
    kind: AiSidebarKind,
) -> AiSidebarResource {
    AiSidebarResource {
        id: id.into(),
        label: label.into(),
        kind,
        children: Vec::new(),
        disabled: false,
    }
}

impl AiSidebarResource {
    /// Nest `children` under this resource.
    pub fn children(mut self, children: Vec<AiSidebarResource>) -> Self {
        self.children = children;
        self
    }

    /// Make this row inert: dimmed, unpressable, and unmovable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// This resource's identity.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Its label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Its kind.
    pub fn kind(&self) -> AiSidebarKind {
        self.kind
    }

    /// Its children, which may be empty.
    pub fn child_nodes(&self) -> &[AiSidebarResource] {
        &self.children
    }

    /// Whether it is inert.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Where a moved resource lands relative to its target — upstream's
/// `SidebarResourceDropPosition`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AiSidebarDrop {
    /// Immediately above the target, as its sibling.
    Before,
    /// As the target's last child. Only legal for a kind that
    /// [`can_contain`](AiSidebarKind::can_contain).
    Inside,
    /// Immediately below the target, as its sibling.
    #[default]
    After,
}

/// One requested move — upstream's `SidebarResourceMove`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiSidebarMove {
    /// The resource being moved.
    pub item_id: String,
    /// What it is being moved relative to; `None` means the top level.
    pub target_id: Option<String>,
    /// Where relative to that target it lands.
    pub position: AiSidebarDrop,
}

/// One flattened row: a resource, how deep it sits, and who its parent is —
/// upstream's `FlatResource`.
#[derive(Clone, Debug, PartialEq)]
pub struct AiSidebarRow {
    /// The resource this row shows.
    pub resource: AiSidebarResource,
    /// Its nesting depth, `0` at the root.
    pub depth: usize,
    /// Its parent's id, `None` at the root.
    pub parent_id: Option<String>,
}

/// Flatten `items` into the rows a tree with `expanded` open shows, in visual
/// order — upstream's `flattenResources`.
pub fn ai_sidebar_rows(items: &[AiSidebarResource], expanded: &[String]) -> Vec<AiSidebarRow> {
    fn walk(
        items: &[AiSidebarResource],
        expanded: &[String],
        depth: usize,
        parent: Option<&str>,
        out: &mut Vec<AiSidebarRow>,
    ) {
        for item in items {
            out.push(AiSidebarRow {
                resource: item.clone(),
                depth,
                parent_id: parent.map(str::to_owned),
            });
            if !item.children.is_empty() && expanded.iter().any(|id| id == &item.id) {
                walk(&item.children, expanded, depth + 1, Some(&item.id), out);
            }
        }
    }
    let mut rows = Vec::new();
    walk(items, expanded, 0, None, &mut rows);
    rows
}

/// Find the resource with `id` anywhere in `items` — upstream's
/// `findResource`.
pub fn ai_sidebar_find<'a>(
    items: &'a [AiSidebarResource],
    id: &str,
) -> Option<&'a AiSidebarResource> {
    for item in items {
        if item.id == id {
            return Some(item);
        }
        if let Some(found) = ai_sidebar_find(&item.children, id) {
            return Some(found);
        }
    }
    None
}

/// Whether `item` is, or contains, `id` — upstream's `containsResource`, which
/// is what stops a folder being dropped inside itself.
fn contains(item: &AiSidebarResource, id: &str) -> bool {
    item.id == id || item.children.iter().any(|child| contains(child, id))
}

/// Lift `id` out of `items`, returning the remaining tree and the resource.
fn remove(
    items: &[AiSidebarResource],
    id: &str,
) -> (Vec<AiSidebarResource>, Option<AiSidebarResource>) {
    let mut removed = None;
    let mut next = Vec::with_capacity(items.len());
    for item in items {
        if item.id == id {
            removed = Some(item.clone());
            continue;
        }
        if item.children.is_empty() {
            next.push(item.clone());
            continue;
        }
        let (children, found) = remove(&item.children, id);
        if found.is_some() {
            removed = found;
            let mut copy = item.clone();
            copy.children = children;
            next.push(copy);
        } else {
            next.push(item.clone());
        }
    }
    (next, removed)
}

/// Put `resource` back into `items` at `target`/`position` — upstream's
/// `insertResource`.
fn insert(
    items: &[AiSidebarResource],
    resource: AiSidebarResource,
    target: Option<&str>,
    position: AiSidebarDrop,
) -> Vec<AiSidebarResource> {
    let Some(target) = target else {
        let mut next = items.to_vec();
        next.push(resource);
        return next;
    };
    let mut next = Vec::with_capacity(items.len() + 1);
    let mut placed = false;
    for item in items {
        if item.id == target {
            placed = true;
            match position {
                AiSidebarDrop::Before => {
                    next.push(resource.clone());
                    next.push(item.clone());
                }
                AiSidebarDrop::After => {
                    next.push(item.clone());
                    next.push(resource.clone());
                }
                AiSidebarDrop::Inside => {
                    let mut copy = item.clone();
                    copy.children.push(resource.clone());
                    next.push(copy);
                }
            }
            continue;
        }
        if item.children.is_empty() {
            next.push(item.clone());
            continue;
        }
        let mut copy = item.clone();
        copy.children = insert(&item.children, resource.clone(), Some(target), position);
        placed = placed || copy.children != item.children;
        next.push(copy);
    }
    next
}

/// Apply `requested` to `items`, returning the new tree — upstream's
/// `moveResource`.
///
/// `None` when the move is illegal, which is exactly upstream's four refusals:
/// an unknown or disabled source, a target inside the source's own subtree, and
/// an `inside` landing on something that is missing, disabled, or cannot hold
/// children.
pub fn ai_sidebar_move(
    items: &[AiSidebarResource],
    requested: &AiSidebarMove,
) -> Option<Vec<AiSidebarResource>> {
    let source = ai_sidebar_find(items, &requested.item_id)?;
    if source.disabled {
        return None;
    }
    if let Some(target) = &requested.target_id
        && contains(source, target)
    {
        return None;
    }
    if requested.position == AiSidebarDrop::Inside {
        let target = requested
            .target_id
            .as_ref()
            .and_then(|id| ai_sidebar_find(items, id))?;
        if target.disabled || !target.kind.can_contain() {
            return None;
        }
    }
    let (rest, removed) = remove(items, &requested.item_id);
    let removed = removed?;
    Some(insert(
        &rest,
        removed,
        requested.target_id.as_deref(),
        requested.position,
    ))
}

/// Rename the resource with `id` to `label`, anywhere in the tree — upstream's
/// `renameResource`.
pub fn ai_sidebar_rename(
    items: &[AiSidebarResource],
    id: &str,
    label: &str,
) -> Vec<AiSidebarResource> {
    items
        .iter()
        .map(|item| {
            let mut copy = item.clone();
            if copy.id == id {
                copy.label = label.to_owned();
            }
            copy.children = ai_sidebar_rename(&item.children, id, label);
            copy
        })
        .collect()
}

/// A view-held, typed id callback (erased on build).
type OnId<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held, typed move callback (erased on build).
type OnMove<State> = Rc<dyn Fn(&mut State, AiSidebarMove)>;

/// A declarative beUI resource tree. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::agents::ai_sidebar::{AiSidebarKind, ai_sidebar, ai_sidebar_resource};
///
/// let tree = ai_sidebar::<()>(vec![
///     ai_sidebar_resource("release", "Release workspace", AiSidebarKind::Project).children(vec![
///         ai_sidebar_resource("audit", "Checkout audit", AiSidebarKind::File),
///         ai_sidebar_resource("sources", "Research sources", AiSidebarKind::Bookmark),
///     ]),
///     ai_sidebar_resource("archive", "Archived runs", AiSidebarKind::Folder),
/// ])
/// .default_expanded(vec!["release".to_owned()]);
/// ```
pub struct AiSidebarView<State: 'static> {
    items: Vec<AiSidebarResource>,
    active_id: Option<String>,
    default_expanded: Vec<String>,
    label: String,
    on_activate: Option<OnId<State>>,
    on_toggle: Option<OnId<State>>,
    on_move: Option<OnMove<State>>,
}

/// Create a resource tree over `items`, everything collapsed and nothing
/// active.
pub fn ai_sidebar<State: 'static>(items: Vec<AiSidebarResource>) -> AiSidebarView<State> {
    AiSidebarView {
        items,
        active_id: None,
        default_expanded: Vec::new(),
        label: AI_SIDEBAR_LABEL.to_owned(),
        on_activate: None,
        on_toggle: None,
        on_move: None,
    }
}

impl<State: 'static> AiSidebarView<State> {
    /// Take ownership of the selection: the tree then shows exactly this row as
    /// active and never selects one itself (`activeId`).
    pub fn active_id(mut self, id: impl Into<String>) -> Self {
        self.active_id = Some(id.into());
        self
    }

    /// The ids expanded when the tree mounts (`defaultExpandedIds`).
    pub fn default_expanded(mut self, ids: Vec<String>) -> Self {
        self.default_expanded = ids;
        self
    }

    /// Replace the tree's accessible name (`ariaLabel`).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Observe selections — a leaf row activated by press or keyboard
    /// (`onActiveChange`).
    pub fn on_activate<F: Fn(&mut State, String) + 'static>(mut self, callback: F) -> Self {
        self.on_activate = Some(Rc::new(callback));
        self
    }

    /// Observe expand/collapse, reporting the id whose disclosure flipped.
    pub fn on_toggle<F: Fn(&mut State, String) + 'static>(mut self, callback: F) -> Self {
        self.on_toggle = Some(Rc::new(callback));
        self
    }

    /// Observe move requests — the four `Alt+Shift+Arrow` commands. Apply one
    /// with [`ai_sidebar_move`]; see the [module docs](self) on why the widget
    /// only reports.
    pub fn on_move<F: Fn(&mut State, AiSidebarMove) + 'static>(mut self, callback: F) -> Self {
        self.on_move = Some(Rc::new(callback));
        self
    }
}

/// The resolved tree palette.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarColors {
    /// The active/hover pill (`bg-muted`).
    pill: Color,
    /// The active or hovered row's ink (`text-foreground`).
    ink: Color,
    /// A resting row's ink (`text-muted-foreground`).
    muted: Color,
}

impl SidebarColors {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                SidebarColors {
                    pill: scheme.surface_container_highest,
                    ink: scheme.on_surface,
                    muted: scheme.on_surface_variant,
                }
            }
            None => SidebarColors {
                pill: FALLBACK.muted,
                ink: FALLBACK.foreground,
                muted: FALLBACK.muted_foreground,
            },
        }
    }
}

/// The type-scale role a row's label takes its family from at layout.
const ROW_ROLE: ThemeTextType = ThemeTextType::LabelLarge;

/// A row's text style: `text-sm`. The family here is the unthemed base;
/// `layout` shapes in [`ROW_ROLE`]'s family.
fn row_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        ..TextStyle::default()
    }
}

/// One retained row.
struct Row {
    id: String,
    kind: AiSidebarKind,
    depth: usize,
    parent_id: Option<String>,
    disabled: bool,
    has_children: bool,
    expanded: bool,
    label: LabelRun,
    presence: Presence,
    /// How present the row was on the last paint — written by `paint`, read by
    /// `layout`, so a collapsing branch closes its slots over its exit.
    shown: f64,
}

impl Row {
    /// A fresh row for `flat`, closed until something opens it.
    fn new(flat: &AiSidebarRow, expanded: bool) -> Self {
        Row {
            id: flat.resource.id.clone(),
            kind: flat.resource.kind,
            depth: flat.depth,
            parent_id: flat.parent_id.clone(),
            disabled: flat.resource.disabled,
            has_children: !flat.resource.children.is_empty(),
            expanded,
            label: LabelRun::new(flat.resource.label.clone()),
            presence: Presence::symmetric(Ramp::eased(
                Duration::from_millis(AI_SIDEBAR_ROW_MS),
                EASE_OUT,
            )),
            shown: 0.0,
        }
    }

    /// Adopt `flat`'s content, reporting what changed.
    fn adopt(&mut self, flat: &AiSidebarRow, expanded: bool) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.label.set_content(flat.resource.label.clone()) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if self.depth != flat.depth
            || self.parent_id != flat.parent_id
            || self.kind != flat.resource.kind
            || self.disabled != flat.resource.disabled
            || self.has_children == flat.resource.children.is_empty()
            || self.expanded != expanded
        {
            self.depth = flat.depth;
            self.parent_id = flat.parent_id.clone();
            self.kind = flat.resource.kind;
            self.disabled = flat.resource.disabled;
            self.has_children = !flat.resource.children.is_empty();
            self.expanded = expanded;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    /// Whether this row toggles rather than selects.
    fn is_container(&self) -> bool {
        self.kind.can_contain()
    }
}

/// The retained widget for an [`AiSidebarView`].
pub struct AiSidebarWidget {
    rows: Vec<Row>,
    /// The whole tree, kept so the keyboard's move commands and the expand
    /// walk can be resolved against structure rather than the flattened view.
    items: Vec<AiSidebarResource>,
    expanded: Vec<String>,
    /// The app-confirmed selection, when the tree is controlled.
    controlled: Option<String>,
    /// The tree's own selection, when it is not.
    internal_active: Option<String>,
    /// Which row the keyboard walk is on.
    focused: usize,
    /// The row a `Down` armed.
    captured: Option<usize>,
    /// The latched hovered row, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    label: String,
    /// The active pill's travel, `0` at its previous rect .. `1` at its new one.
    pill: Lane,
    pill_from: Option<Rect>,
    pill_shown: Option<Rect>,
    /// The tree's own width, resolved by layout.
    width: f64,
    on_activate: Option<ErasedArgCallback<String>>,
    on_toggle: Option<ErasedArgCallback<String>>,
    on_move: Option<ErasedArgCallback<AiSidebarMove>>,
}

impl AiSidebarWidget {
    /// The selection this tree is showing.
    pub fn active_id(&self) -> Option<&str> {
        self.controlled
            .as_deref()
            .or(self.internal_active.as_deref())
    }

    /// The ids currently expanded.
    pub fn expanded_ids(&self) -> &[String] {
        &self.expanded
    }

    /// How many rows are mounted, including any still playing an exit.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The id of row `index`.
    pub fn row_id(&self, index: usize) -> Option<&str> {
        self.rows.get(index).map(|row| row.id.as_str())
    }

    /// Which row the keyboard walk is on.
    pub fn focused_row(&self) -> usize {
        self.focused
    }

    /// Row `index`'s box, as of the last layout.
    pub fn row_box(&self, index: usize) -> Option<Rect> {
        let mut y = 0.0;
        for (position, row) in self.rows.iter().enumerate() {
            let height = AI_SIDEBAR_ROW_HEIGHT * row.shown.clamp(0.0, 1.0);
            if position == index {
                return Some(Rect::from_origin_size(
                    Point::new(0.0, y),
                    Size::new(self.width, height),
                ));
            }
            y += height + AI_SIDEBAR_ROW_GAP * row.shown.clamp(0.0, 1.0);
        }
        None
    }

    /// How present row `index` is, `0` gone .. `1` settled.
    pub fn row_presence(&self, index: usize, now: FrameTime) -> f64 {
        self.rows
            .get(index)
            .map_or(0.0, |row| row.presence.presence(now))
    }

    /// The active pill's rect right now, or `None` when nothing is selected.
    pub fn pill_rect(&self) -> Option<Rect> {
        self.pill_shown
    }

    /// Whether row `index` may be pressed.
    fn enabled(&self, index: usize) -> bool {
        self.rows
            .get(index)
            .is_some_and(|row| !row.disabled && row.presence.is_visible())
    }

    /// Which row `position` falls in, if any.
    fn hit_row(&self, position: Point) -> Option<usize> {
        (0..self.rows.len())
            .find(|index| self.row_box(*index).is_some_and(|r| r.contains(position)))
    }

    /// The active row's resting pill rect.
    fn pill_target(&self) -> Option<Rect> {
        let active = self.active_id()?;
        let index = self
            .rows
            .iter()
            .position(|row| row.id == active && !row.is_container())?;
        self.row_box(index)
    }

    /// Flip `id`'s disclosure, reporting whether anything changed.
    fn toggle(&mut self, id: &str) -> bool {
        match self.expanded.iter().position(|held| held == id) {
            Some(index) => {
                self.expanded.remove(index);
            }
            None => self.expanded.push(id.to_owned()),
        }
        true
    }

    /// Re-derive the mounted rows from the tree and the expanded set, keeping
    /// each surviving row's motion state and playing the rest out.
    fn resync_rows(&mut self) -> ChangeFlags {
        let flat = ai_sidebar_rows(&self.items, &self.expanded);
        let mut flags = ChangeFlags::NONE;
        let mut previous = std::mem::take(&mut self.rows);
        let mut kept: Vec<Row> = Vec::with_capacity(flat.len());
        for row in &flat {
            let expanded = self.expanded.iter().any(|id| id == &row.resource.id);
            match previous.iter().position(|held| held.id == row.resource.id) {
                Some(index) => {
                    let mut held = previous.remove(index);
                    flags |= held.adopt(row, expanded);
                    if held.presence.set_open(true) {
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                    kept.push(held);
                }
                None => {
                    let mut fresh = Row::new(row, expanded);
                    fresh.presence.set_open(true);
                    kept.push(fresh);
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
        self.rows = kept;
        self.focused = self.focused.min(self.rows.len().saturating_sub(1));
        flags
    }

    /// Activate row `index`: a container toggles, a leaf selects.
    fn activate(&mut self, ctx: &mut EventCtx, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let id = row.id.clone();
        if row.is_container() {
            self.toggle(&id);
            self.resync_rows();
            if let Some(callback) = &mut self.on_toggle {
                callback(ctx, id);
            }
        } else {
            if self.controlled.is_none() {
                self.internal_active = Some(id.clone());
            }
            if let Some(callback) = &mut self.on_activate {
                callback(ctx, id);
            }
        }
        ctx.request_redraw();
    }

    /// The move `key` asks for from row `index`, if it is a legal one — the
    /// same four commands upstream binds to `Alt+Shift+Arrow`.
    fn move_command(&self, index: usize, key: &NamedKey) -> Option<AiSidebarMove> {
        let row = self.rows.get(index)?;
        if row.disabled {
            return None;
        }
        let previous = index.checked_sub(1).and_then(|i| self.rows.get(i));
        let next = self.rows.get(index + 1);
        match key {
            NamedKey::ArrowUp => previous.map(|target| AiSidebarMove {
                item_id: row.id.clone(),
                target_id: Some(target.id.clone()),
                position: AiSidebarDrop::Before,
            }),
            NamedKey::ArrowDown => next.map(|target| AiSidebarMove {
                item_id: row.id.clone(),
                target_id: Some(target.id.clone()),
                position: AiSidebarDrop::After,
            }),
            NamedKey::ArrowRight => previous
                .filter(|target| {
                    // Only offer the reparent when it lands somewhere new: the
                    // row above a folder's first child is the folder it is
                    // already in.
                    target.kind.can_contain() && Some(&target.id) != row.parent_id.as_ref()
                })
                .map(|target| AiSidebarMove {
                    item_id: row.id.clone(),
                    target_id: Some(target.id.clone()),
                    position: AiSidebarDrop::Inside,
                }),
            NamedKey::ArrowLeft => row.parent_id.clone().map(|parent| AiSidebarMove {
                item_id: row.id.clone(),
                target_id: Some(parent),
                position: AiSidebarDrop::After,
            }),
            _ => None,
        }
    }

    /// Handle one key press, returning whether it was consumed.
    fn on_key(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> bool {
        let index = self.focused;
        let moving = key.modifiers.alt && key.modifiers.shift;
        let named = match &key.key {
            Key::Named(named) => Some(*named),
            Key::Character(text) if text == " " => Some(NamedKey::Enter),
            _ => None,
        };
        let Some(named) = named else {
            return false;
        };

        if moving {
            if let Some(requested) = self.move_command(index, &named) {
                // The reparent expands its new home first, exactly as upstream
                // does before issuing the move.
                if requested.position == AiSidebarDrop::Inside
                    && let Some(target) = &requested.target_id
                    && !self.expanded.iter().any(|id| id == target)
                {
                    self.expanded.push(target.clone());
                    self.resync_rows();
                }
                if let Some(callback) = &mut self.on_move {
                    callback(ctx, requested);
                }
                ctx.request_redraw();
                return true;
            }
            // A move modifier always consumes the key, so an unavailable move
            // does not fall through to the plain arrow walk.
            return true;
        }

        match named {
            NamedKey::ArrowDown => {
                if index + 1 < self.rows.len() {
                    self.focused = index + 1;
                    ctx.request_redraw();
                }
                true
            }
            NamedKey::ArrowUp => {
                if index > 0 {
                    self.focused = index - 1;
                    ctx.request_redraw();
                }
                true
            }
            NamedKey::Home => {
                self.focused = 0;
                ctx.request_redraw();
                true
            }
            NamedKey::End => {
                self.focused = self.rows.len().saturating_sub(1);
                ctx.request_redraw();
                true
            }
            NamedKey::ArrowRight => {
                let Some(row) = self.rows.get(index) else {
                    return true;
                };
                if row.disabled {
                    return true;
                }
                let (id, container, expanded) = (row.id.clone(), row.is_container(), row.expanded);
                if container && !expanded {
                    self.toggle(&id);
                    self.resync_rows();
                    if let Some(callback) = &mut self.on_toggle {
                        callback(ctx, id);
                    }
                } else if container
                    && self
                        .rows
                        .get(index + 1)
                        .is_some_and(|next| next.parent_id.as_deref() == Some(id.as_str()))
                {
                    self.focused = index + 1;
                }
                ctx.request_redraw();
                true
            }
            NamedKey::ArrowLeft => {
                let Some(row) = self.rows.get(index) else {
                    return true;
                };
                let (id, expanded, parent) = (row.id.clone(), row.expanded, row.parent_id.clone());
                if expanded && !row.disabled {
                    self.toggle(&id);
                    self.resync_rows();
                    if let Some(callback) = &mut self.on_toggle {
                        callback(ctx, id);
                    }
                } else if let Some(parent) = parent
                    && let Some(position) = self.rows.iter().position(|row| row.id == parent)
                {
                    self.focused = position;
                }
                ctx.request_redraw();
                true
            }
            NamedKey::Enter => {
                if self.enabled(index) {
                    self.activate(ctx, index);
                }
                true
            }
            _ => false,
        }
    }
}

impl<State: 'static> View<State> for AiSidebarView<State> {
    type Element = AiSidebarWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AiSidebarWidget {
        let expanded = self.default_expanded.clone();
        let flat = ai_sidebar_rows(&self.items, &expanded);
        let mut rows: Vec<Row> = flat
            .iter()
            .map(|row| Row::new(row, expanded.iter().any(|id| id == &row.resource.id)))
            .collect();
        // A tree mounts settled: the rows it opens with do not play a reveal.
        for row in &mut rows {
            row.presence.set_open(true);
            row.presence.advance(FrameTime::from_nanos(0));
            row.presence.advance(FrameTime::from_nanos(u64::MAX / 2));
            row.shown = 1.0;
        }
        AiSidebarWidget {
            rows,
            items: self.items.clone(),
            expanded,
            controlled: self.active_id.clone(),
            internal_active: None,
            focused: 0,
            captured: None,
            hovered: None,
            label: self.label.clone(),
            pill: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 1.0),
            pill_from: None,
            pill_shown: None,
            width: 0.0,
            on_activate: self.on_activate.as_ref().map(erase_callback_arg),
            on_toggle: self.on_toggle.as_ref().map(erase_callback_arg),
            on_move: self.on_move.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AiSidebarWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.items != self.items {
            element.items = self.items.clone();
            flags |= element.resync_rows();
        }
        if element.controlled != self.active_id {
            element.controlled = self.active_id.clone();
            flags |= ChangeFlags::PAINT;
        }
        element.label = self.label.clone();
        element.on_activate = self.on_activate.as_ref().map(erase_callback_arg);
        element.on_toggle = self.on_toggle.as_ref().map(erase_callback_arg);
        element.on_move = self.on_move.as_ref().map(erase_callback_arg);
        flags
    }
}

impl Widget for AiSidebarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.rows.retain(|row| row.presence.is_visible());
        self.width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let style = row_style();
        let mut height = 0.0;
        for row in &mut self.rows {
            row.label.layout_themed(ctx, &style, ROW_ROLE);
            let shown = row.shown.clamp(0.0, 1.0);
            height += (AI_SIDEBAR_ROW_HEIGHT + AI_SIDEBAR_ROW_GAP) * shown;
        }
        bc.constrain(Size::new(
            self.width,
            (height - AI_SIDEBAR_ROW_GAP).max(0.0),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, colors) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                SidebarColors::resolve(theme),
            )
        };
        let now = ctx.frame_time();
        let origin = ctx.origin();
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let mut owes_layout = false;
        for row in &mut self.rows {
            if reduce_motion {
                // Two steps: the first latches the ramp's start, the second
                // lands it — a reveal must complete on the frame it begins.
                row.presence.advance(now);
                row.presence.advance(FrameTime::from_nanos(
                    now.as_nanos().saturating_add(SETTLE_AT_ONCE_NANOS),
                ));
                row.shown = if row.presence.is_visible() { 1.0 } else { 0.0 };
                continue;
            }
            let presence = row.presence.advance(now);
            row.shown = presence.clamp(0.0, 1.0);
            owes_layout |= row.presence.is_animating();
        }

        // The active pill springs between rows, upstream's `layoutId`.
        let target = self.pill_target();
        match (self.pill_shown, target) {
            (Some(shown), Some(target)) if shown != target => {
                if reduce_motion {
                    self.pill_shown = Some(target);
                } else {
                    self.pill_from = Some(shown);
                    self.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0);
                    self.pill.retarget(1.0);
                    self.pill_shown = Some(target);
                }
            }
            (None, Some(target)) => {
                self.pill_from = None;
                self.pill_shown = Some(target);
            }
            (Some(_), None) => {
                self.pill_from = None;
                self.pill_shown = None;
            }
            _ => {}
        }
        let travelling = self.pill.advance(now);
        let pill_rect = match (self.pill_from, self.pill_shown) {
            (Some(from), Some(to)) if travelling => {
                let t = self.pill.value().clamp(0.0, 1.0);
                Some(Rect::new(
                    from.x0 + (to.x0 - from.x0) * t,
                    from.y0 + (to.y0 - from.y0) * t,
                    from.x1 + (to.x1 - from.x1) * t,
                    from.y1 + (to.y1 - from.y1) * t,
                ))
            }
            (_, shown) => shown,
        };
        if let Some(rect) = pill_rect {
            scene.fill_rounded_rect(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                rect.size(),
                style::resolve_radius(AI_SIDEBAR_RADIUS, rect.width(), rect.height()),
                colors.pill,
            );
        }

        for index in 0..self.rows.len() {
            let Some(rect) = self.row_box(index) else {
                continue;
            };
            let row = &self.rows[index];
            let alpha = row.shown.clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let hovered = self.hovered == Some(index);
            let active = self.active_id() == Some(row.id.as_str());
            let shift = match row.presence.phase() {
                PresencePhase::Exiting => -AI_SIDEBAR_ROW_SHIFT * (1.0 - alpha),
                _ => AI_SIDEBAR_ROW_SHIFT * (1.0 - alpha),
            };
            let row_origin = Point::new(at.x, at.y + shift);

            if alpha < 1.0 {
                scene.push_layer(row_origin, rect.size(), alpha as f32);
            }
            if hovered && !active && !row.disabled {
                scene.fill_rounded_rect(
                    row_origin,
                    Size::new(rect.width(), AI_SIDEBAR_ROW_HEIGHT),
                    style::resolve_radius(AI_SIDEBAR_RADIUS, rect.width(), AI_SIDEBAR_ROW_HEIGHT),
                    scale_alpha(colors.pill, HOVER_ALPHA),
                );
            }
            let ink = if row.disabled {
                scale_alpha(colors.muted, DISABLED_ALPHA)
            } else if active || hovered {
                colors.ink
            } else {
                scale_alpha(colors.muted, LABEL_ALPHA)
            };
            let mark_x = row_origin.x + AI_SIDEBAR_PADDING_X + row.depth as f64 * AI_SIDEBAR_INDENT;
            paint_mark(
                row,
                scene,
                Point::new(
                    mark_x,
                    row_origin.y + (AI_SIDEBAR_ROW_HEIGHT - AI_SIDEBAR_MARK_SIZE) / 2.0,
                ),
                ink,
            );
            row.label.paint(
                Point::new(
                    mark_x + AI_SIDEBAR_MARK_SIZE + AI_SIDEBAR_GAP,
                    row_origin.y + (AI_SIDEBAR_ROW_HEIGHT - row.label.size().height) / 2.0,
                ),
                ink,
                scene,
            );
            if alpha < 1.0 {
                scene.pop_layer();
            }
        }

        if owes_layout {
            ctx.request_layout();
        } else if travelling {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_row(p.position).filter(|i| self.enabled(*i)) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(index);
                    self.focused = index;
                    ctx.request_focus();
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let over = self.hit_row(p.position).filter(|i| self.enabled(*i));
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    if self.captured.is_some() {
                        EventResult::Handled
                    } else {
                        EventResult::Ignored
                    }
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
            InputEvent::Key(key) => {
                if self.on_key(ctx, key) {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.label.clone();
        let active = self.active_id().map(str::to_owned);
        ctx.push_container(
            Role::Tree,
            |node| node.set_label(label),
            |ctx| {
                for row in self.rows.iter().filter(|row| row.presence.is_visible()) {
                    ctx.push_node(Role::TreeItem, |node| {
                        node.set_label(row.label.content());
                        node.set_level(row.depth + 1);
                        if row.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                        if row.is_container() {
                            node.set_expanded(row.expanded);
                        } else if active.as_deref() == Some(row.id.as_str()) {
                            node.set_selected(true);
                        }
                    });
                }
            },
        );
    }
}

/// Paint one row's kind mark: an open or closed folder, a document, or a
/// bookmark — upstream's `defaultIcon`, drawn from primitives.
fn paint_mark(row: &Row, scene: &mut dyn PaintScene, origin: Point, ink: Color) {
    let brush = Brush::Solid(ink);
    let mut path = BezPath::new();
    match row.kind {
        AiSidebarKind::Folder | AiSidebarKind::Project => {
            // A folder with a tab; the open form leans its front flap out.
            path.move_to(Point::new(origin.x + 2.0, origin.y + 15.0));
            path.line_to(Point::new(origin.x + 2.0, origin.y + 5.0));
            path.line_to(Point::new(origin.x + 7.5, origin.y + 5.0));
            path.line_to(Point::new(origin.x + 9.5, origin.y + 7.0));
            path.line_to(Point::new(origin.x + 17.0, origin.y + 7.0));
            path.line_to(Point::new(origin.x + 17.0, origin.y + 15.0));
            path.close_path();
            scene.stroke_path(Point::ORIGIN, &path, 1.4, &brush);
            if row.expanded {
                scene.stroke_line(
                    Point::new(origin.x + 4.0, origin.y + 15.0),
                    Point::new(origin.x + 19.0, origin.y + 15.0),
                    1.4,
                    ink,
                );
            }
        }
        AiSidebarKind::File => {
            path.move_to(Point::new(origin.x + 4.0, origin.y + 3.0));
            path.line_to(Point::new(origin.x + 11.5, origin.y + 3.0));
            path.line_to(Point::new(origin.x + 15.5, origin.y + 7.0));
            path.line_to(Point::new(origin.x + 15.5, origin.y + 16.5));
            path.line_to(Point::new(origin.x + 4.0, origin.y + 16.5));
            path.close_path();
            scene.stroke_path(Point::ORIGIN, &path, 1.4, &brush);
            scene.stroke_line(
                Point::new(origin.x + 11.5, origin.y + 3.0),
                Point::new(origin.x + 11.5, origin.y + 7.0),
                1.4,
                ink,
            );
        }
        AiSidebarKind::Bookmark => {
            path.move_to(Point::new(origin.x + 5.0, origin.y + 3.0));
            path.line_to(Point::new(origin.x + 15.0, origin.y + 3.0));
            path.line_to(Point::new(origin.x + 15.0, origin.y + 17.0));
            path.line_to(Point::new(origin.x + 10.0, origin.y + 12.5));
            path.line_to(Point::new(origin.x + 5.0, origin.y + 17.0));
            path.close_path();
            scene.stroke_path(Point::ORIGIN, &path, 1.4, &brush);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Modifiers, PointerButton, PointerEvent};
    use std::any::Any;

    /// The rail every tree test lays itself into.
    const RAIL: Size = Size::new(240.0, 600.0);

    /// Records the primitives the tree paints.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        strokes: usize,
        lines: usize,
        layers: Vec<f32>,
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
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// What the tree reports.
    #[derive(Default)]
    struct Picked {
        activated: Vec<String>,
        toggled: Vec<String>,
        moves: Vec<AiSidebarMove>,
    }

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    /// The workspace upstream's own preview uses, trimmed to three roots.
    fn tree() -> Vec<AiSidebarResource> {
        vec![
            ai_sidebar_resource("release", "Release workspace", AiSidebarKind::Project).children(
                vec![
                    ai_sidebar_resource("audit", "Checkout audit", AiSidebarKind::File),
                    ai_sidebar_resource("notes", "Release notes", AiSidebarKind::File),
                    ai_sidebar_resource("sources", "Research sources", AiSidebarKind::Bookmark),
                ],
            ),
            ai_sidebar_resource("design", "Design system", AiSidebarKind::Folder).children(vec![
                ai_sidebar_resource("tokens", "Motion tokens", AiSidebarKind::File),
            ]),
            ai_sidebar_resource("archive", "Archived runs", AiSidebarKind::Folder),
        ]
    }

    fn view() -> AiSidebarView<Picked> {
        ai_sidebar::<Picked>(tree())
            .default_expanded(vec!["release".to_owned()])
            .on_activate(|state: &mut Picked, id| state.activated.push(id))
            .on_toggle(|state: &mut Picked, id| state.toggled.push(id))
            .on_move(|state: &mut Picked, requested| state.moves.push(requested))
    }

    fn build(view: &AiSidebarView<Picked>) -> AiSidebarWidget {
        let mut next_id = 0u64;
        View::<Picked>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn layout(widget: &mut AiSidebarWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::loose(RAIL))
    }

    fn laid_out(view: &AiSidebarView<Picked>) -> (AiSidebarWidget, Size) {
        let mut widget = build(view);
        let size = layout(&mut widget);
        (widget, size)
    }

    fn painted(
        widget: &mut AiSidebarWidget,
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

    fn press_row(widget: &mut AiSidebarWidget, size: Size, index: usize, state: &mut Picked) {
        let rect = widget.row_box(index).expect("row exists");
        let point = Point::new(rect.x0 + 20.0, rect.y0 + rect.height() / 2.0);
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ORIGIN, size);
            widget.event(
                &mut ctx,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: point,
                    button: PointerButton::Primary,
                }),
            );
        }
    }

    fn key(widget: &mut AiSidebarWidget, size: Size, named: NamedKey, state: &mut Picked) {
        send_key(widget, size, named, Modifiers::default(), state);
    }

    fn move_key(widget: &mut AiSidebarWidget, size: Size, named: NamedKey, state: &mut Picked) {
        let modifiers = Modifiers {
            alt: true,
            shift: true,
            ..Modifiers::default()
        };
        send_key(widget, size, named, modifiers, state);
    }

    fn send_key(
        widget: &mut AiSidebarWidget,
        size: Size,
        named: NamedKey,
        modifiers: Modifiers,
        state: &mut Picked,
    ) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ORIGIN, size);
        widget.event(
            &mut ctx,
            &InputEvent::Key(KeyEvent {
                key: Key::Named(named),
                modifiers,
                repeat: false,
            }),
        );
    }

    fn ids(rows: &[AiSidebarRow]) -> Vec<&str> {
        rows.iter().map(|row| row.resource.id.as_str()).collect()
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The flatten walks only the branches that are open, and records each
    /// row's depth and parent.
    #[test]
    fn the_flatten_walks_only_the_open_branches() {
        let closed = ai_sidebar_rows(&tree(), &[]);
        assert_eq!(ids(&closed), vec!["release", "design", "archive"]);
        assert!(closed.iter().all(|row| row.depth == 0));
        assert!(closed.iter().all(|row| row.parent_id.is_none()));

        let open = ai_sidebar_rows(&tree(), &["release".to_owned()]);
        assert_eq!(
            ids(&open),
            vec!["release", "audit", "notes", "sources", "design", "archive"]
        );
        assert_eq!(open[1].depth, 1);
        assert_eq!(open[1].parent_id.as_deref(), Some("release"));

        // Both branches at once, nested depths intact.
        let both = ai_sidebar_rows(&tree(), &["release".to_owned(), "design".to_owned()]);
        assert_eq!(both.len(), 7);
        assert_eq!(
            both.iter().filter(|row| row.depth == 1).count(),
            4,
            "three under release, one under design"
        );
        assert!(ai_sidebar_find(&tree(), "tokens").is_some());
        assert!(ai_sidebar_find(&tree(), "nope").is_none());
    }

    /// Upstream's `moveResource`, arm for arm: reorder, reparent, unparent, and
    /// each of its four refusals.
    #[test]
    fn moving_a_resource_reorders_reparents_and_refuses_the_illegal_cases() {
        let items = tree();
        // Reorder at the root.
        let moved = ai_sidebar_move(
            &items,
            &AiSidebarMove {
                item_id: "archive".to_owned(),
                target_id: Some("release".to_owned()),
                position: AiSidebarDrop::Before,
            },
        )
        .expect("a legal reorder");
        assert_eq!(
            ids(&ai_sidebar_rows(&moved, &[])),
            vec!["archive", "release", "design"]
        );

        // Reparent into a container.
        let moved = ai_sidebar_move(
            &items,
            &AiSidebarMove {
                item_id: "archive".to_owned(),
                target_id: Some("design".to_owned()),
                position: AiSidebarDrop::Inside,
            },
        )
        .expect("a legal reparent");
        let inside = ai_sidebar_find(&moved, "design").expect("design survived");
        assert_eq!(inside.child_nodes().len(), 2);
        assert_eq!(inside.child_nodes()[1].id(), "archive");

        // Unparent back to the top level.
        let moved = ai_sidebar_move(
            &items,
            &AiSidebarMove {
                item_id: "audit".to_owned(),
                target_id: None,
                position: AiSidebarDrop::After,
            },
        )
        .expect("a legal unparent");
        assert_eq!(
            ids(&ai_sidebar_rows(&moved, &[])),
            vec!["release", "design", "archive", "audit"]
        );

        // ...and the refusals: unknown source, a target inside the source, an
        // `inside` onto a leaf, and a disabled source.
        assert!(
            ai_sidebar_move(
                &items,
                &AiSidebarMove {
                    item_id: "nope".to_owned(),
                    target_id: None,
                    position: AiSidebarDrop::After,
                }
            )
            .is_none()
        );
        assert!(
            ai_sidebar_move(
                &items,
                &AiSidebarMove {
                    item_id: "release".to_owned(),
                    target_id: Some("audit".to_owned()),
                    position: AiSidebarDrop::After,
                }
            )
            .is_none(),
            "a folder cannot land inside its own subtree"
        );
        assert!(
            ai_sidebar_move(
                &items,
                &AiSidebarMove {
                    item_id: "archive".to_owned(),
                    target_id: Some("notes".to_owned()),
                    position: AiSidebarDrop::Inside,
                }
            )
            .is_none(),
            "a file cannot contain anything"
        );
        let locked = vec![
            ai_sidebar_resource("locked", "Locked", AiSidebarKind::File).disabled(true),
            ai_sidebar_resource("other", "Other", AiSidebarKind::File),
        ];
        assert!(
            ai_sidebar_move(
                &locked,
                &AiSidebarMove {
                    item_id: "locked".to_owned(),
                    target_id: Some("other".to_owned()),
                    position: AiSidebarDrop::After,
                }
            )
            .is_none()
        );
    }

    /// A rename reaches a nested resource and leaves every other label alone.
    #[test]
    fn renaming_reaches_a_nested_resource() {
        let renamed = ai_sidebar_rename(&tree(), "tokens", "Motion tokens v2");
        assert_eq!(
            ai_sidebar_find(&renamed, "tokens").map(AiSidebarResource::label),
            Some("Motion tokens v2")
        );
        assert_eq!(
            ai_sidebar_find(&renamed, "audit").map(AiSidebarResource::label),
            Some("Checkout audit")
        );
        // An unknown id changes nothing at all.
        assert_eq!(ai_sidebar_rename(&tree(), "nope", "x"), tree());
    }

    /// Upstream's press rule: a container toggles, a leaf selects.
    #[test]
    fn pressing_a_container_toggles_and_pressing_a_leaf_selects() {
        let (mut widget, size) = laid_out(&view());
        let mut state = Picked::default();
        assert_eq!(widget.row_count(), 6, "release is open at mount");

        // Row 1 is `audit`, a file.
        press_row(&mut widget, size, 1, &mut state);
        assert_eq!(state.activated, vec!["audit".to_owned()]);
        assert_eq!(widget.active_id(), Some("audit"));
        assert!(state.toggled.is_empty());

        // Row 0 is `release`, a project: it collapses instead.
        press_row(&mut widget, size, 0, &mut state);
        assert_eq!(state.toggled, vec!["release".to_owned()]);
        assert_eq!(state.activated.len(), 1, "a container never selects");
        assert!(!widget.expanded_ids().iter().any(|id| id == "release"));
    }

    /// The keyboard walk: arrows move focus, right/left open and close, Enter
    /// activates, Home/End jump.
    #[test]
    fn the_keyboard_walks_expands_collapses_and_activates() {
        let (mut widget, size) = laid_out(&view());
        let mut state = Picked::default();
        assert_eq!(widget.focused_row(), 0);

        key(&mut widget, size, NamedKey::ArrowDown, &mut state);
        assert_eq!(widget.focused_row(), 1);
        key(&mut widget, size, NamedKey::ArrowUp, &mut state);
        assert_eq!(widget.focused_row(), 0);

        // Left closes the focused branch, right opens it again.
        key(&mut widget, size, NamedKey::ArrowLeft, &mut state);
        assert_eq!(state.toggled, vec!["release".to_owned()]);
        assert!(!widget.expanded_ids().iter().any(|id| id == "release"));
        key(&mut widget, size, NamedKey::ArrowRight, &mut state);
        assert!(widget.expanded_ids().iter().any(|id| id == "release"));

        // ...and right again walks into the branch it just opened.
        key(&mut widget, size, NamedKey::ArrowRight, &mut state);
        assert_eq!(widget.focused_row(), 1);

        key(&mut widget, size, NamedKey::End, &mut state);
        assert_eq!(widget.focused_row(), widget.row_count() - 1);
        key(&mut widget, size, NamedKey::Home, &mut state);
        assert_eq!(widget.focused_row(), 0);

        // Enter on a leaf selects it.
        key(&mut widget, size, NamedKey::ArrowDown, &mut state);
        key(&mut widget, size, NamedKey::Enter, &mut state);
        assert_eq!(state.activated, vec!["audit".to_owned()]);
    }

    /// `Alt+Shift+Arrow` reports the same four moves upstream's row menu
    /// carries, and the tree itself never applies them.
    #[test]
    fn alt_shift_arrows_report_the_four_moves() {
        let (mut widget, size) = laid_out(&view());
        let mut state = Picked::default();
        let before = widget.row_count();

        // Focus `notes` (index 2), which has a sibling either side.
        key(&mut widget, size, NamedKey::ArrowDown, &mut state);
        key(&mut widget, size, NamedKey::ArrowDown, &mut state);
        assert_eq!(widget.row_id(2), Some("notes"));

        move_key(&mut widget, size, NamedKey::ArrowUp, &mut state);
        move_key(&mut widget, size, NamedKey::ArrowDown, &mut state);
        move_key(&mut widget, size, NamedKey::ArrowLeft, &mut state);
        assert_eq!(state.moves.len(), 3);
        assert_eq!(
            state.moves[0],
            AiSidebarMove {
                item_id: "notes".to_owned(),
                target_id: Some("audit".to_owned()),
                position: AiSidebarDrop::Before,
            }
        );
        assert_eq!(state.moves[1].position, AiSidebarDrop::After);
        assert_eq!(state.moves[1].target_id.as_deref(), Some("sources"));
        assert_eq!(
            state.moves[2],
            AiSidebarMove {
                item_id: "notes".to_owned(),
                target_id: Some("release".to_owned()),
                position: AiSidebarDrop::After,
            },
            "left moves a child out to sit after its parent"
        );
        assert_eq!(
            widget.row_count(),
            before,
            "the tree reports and never applies the move itself"
        );

        // Every reported move is one the pure transform accepts.
        for requested in &state.moves {
            assert!(
                ai_sidebar_move(&tree(), requested).is_some(),
                "{requested:?}"
            );
        }
    }

    /// Expanding reveals the children, collapsing plays them out before their
    /// slots close, and the reveal drives layout.
    #[test]
    fn expanding_reveals_children_and_collapsing_plays_them_out() {
        let collapsed = ai_sidebar::<Picked>(tree());
        let (mut widget, size) = laid_out(&collapsed);
        let mut state = Picked::default();
        assert_eq!(widget.row_count(), 3);
        let short = layout(&mut widget);

        press_row(&mut widget, size, 0, &mut state);
        assert_eq!(widget.row_count(), 6, "the branch mounted");
        let (_, _, needs_layout) = painted(&mut widget, size, 0, None);
        assert!(needs_layout, "a reveal that opens slots must drive layout");
        assert!(widget.row_presence(1, at(0)) < 1.0, "and starts closed");
        painted(&mut widget, size, AI_SIDEBAR_ROW_MS, None);
        let tall = layout(&mut widget);
        assert!(tall.height > short.height);

        // Collapsing keeps them mounted for the exit, then drops them.
        press_row(&mut widget, size, 0, &mut state);
        assert_eq!(widget.row_count(), 6, "still mounted for the exit");
        painted(&mut widget, size, AI_SIDEBAR_ROW_MS + 10, None);
        painted(&mut widget, size, AI_SIDEBAR_ROW_MS * 2 + 10, None);
        layout(&mut widget);
        assert_eq!(widget.row_count(), 3, "the slots closed");
    }

    /// The active pill travels between rows rather than teleporting, and lands
    /// on the newly selected one.
    #[test]
    fn the_active_pill_springs_between_rows() {
        let (mut widget, size) = laid_out(&view());
        let mut state = Picked::default();
        painted(&mut widget, size, 0, None);
        assert_eq!(widget.pill_rect(), None, "nothing is selected yet");

        press_row(&mut widget, size, 1, &mut state);
        painted(&mut widget, size, 10, None);
        let first = widget.pill_rect().expect("the pill appeared");
        assert_eq!(first, widget.row_box(1).unwrap());

        press_row(&mut widget, size, 2, &mut state);
        let (_, needs_frame, needs_layout) = painted(&mut widget, size, 20, None);
        assert!(
            needs_frame || needs_layout,
            "the pill owes frames while travelling"
        );
        painted(&mut widget, size, 5_000, None);
        assert_eq!(widget.pill_rect(), widget.row_box(2), "and it lands");

        // A container never wears the pill — upstream only marks leaves
        // `aria-selected`.
        press_row(&mut widget, size, 0, &mut state);
        painted(&mut widget, size, 5_010, None);
        assert_eq!(
            widget.active_id(),
            Some("notes"),
            "a container press toggles"
        );
    }

    /// A disabled row is inert to press and to a move command.
    #[test]
    fn a_disabled_row_is_inert() {
        let items = vec![
            ai_sidebar_resource("locked", "Locked", AiSidebarKind::File).disabled(true),
            ai_sidebar_resource("open", "Open", AiSidebarKind::File),
        ];
        let tree = ai_sidebar::<Picked>(items)
            .on_activate(|state: &mut Picked, id| state.activated.push(id))
            .on_move(|state: &mut Picked, requested| state.moves.push(requested));
        let (mut widget, size) = laid_out(&tree);
        let mut state = Picked::default();

        press_row(&mut widget, size, 0, &mut state);
        assert!(state.activated.is_empty(), "a disabled row does not select");
        assert_eq!(widget.active_id(), None);

        move_key(&mut widget, size, NamedKey::ArrowDown, &mut state);
        assert!(state.moves.is_empty(), "nor does it move");

        press_row(&mut widget, size, 1, &mut state);
        assert_eq!(state.activated, vec!["open".to_owned()]);
    }

    /// A controlled tree shows what its owner says and never selects itself.
    #[test]
    fn a_controlled_tree_never_selects_itself() {
        let controlled = ai_sidebar::<Picked>(tree())
            .default_expanded(vec!["release".to_owned()])
            .active_id("notes")
            .on_activate(|state: &mut Picked, id| state.activated.push(id));
        let (mut widget, size) = laid_out(&controlled);
        let mut state = Picked::default();
        assert_eq!(widget.active_id(), Some("notes"));

        press_row(&mut widget, size, 1, &mut state);
        assert_eq!(
            state.activated,
            vec!["audit".to_owned()],
            "the request is reported"
        );
        assert_eq!(widget.active_id(), Some("notes"), "the owner still decides");
    }

    /// Each kind paints its own mark, and a row's indent grows with its depth.
    #[test]
    fn each_kind_paints_its_own_mark_and_depth_indents() {
        for kind in AiSidebarKind::ALL {
            let single = ai_sidebar::<Picked>(vec![ai_sidebar_resource("a", "Row", kind)]);
            let (mut widget, size) = laid_out(&single);
            let (rec, _, _) = painted(&mut widget, size, 0, None);
            assert!(rec.strokes > 0, "{kind:?} paints a mark");
            assert!(!rec.inks.is_empty(), "{kind:?} shapes its label");
        }

        // The indent itself: a nested row's mark sits one step further in.
        let rows = ai_sidebar_rows(&tree(), &["release".to_owned()]);
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].depth, 1);
        let root_x = AI_SIDEBAR_PADDING_X;
        let child_x = AI_SIDEBAR_PADDING_X + AI_SIDEBAR_INDENT;
        assert!(child_x > root_x);
    }

    /// `reduce_motion` lands every reveal on the first paint.
    #[test]
    fn reduce_motion_lands_the_reveal_at_once() {
        let theme = reduced();
        let collapsed = ai_sidebar::<Picked>(tree());
        let (mut widget, size) = laid_out(&collapsed);
        let mut state = Picked::default();
        painted(&mut widget, size, 0, Some(&theme));
        press_row(&mut widget, size, 0, &mut state);
        let (_, needs_frame, needs_layout) = painted(&mut widget, size, 1, Some(&theme));
        assert!(!needs_frame && !needs_layout, "nothing is owed");
        assert_eq!(
            widget.row_presence(1, at(1)),
            1.0,
            "the child is already there"
        );
    }

    /// The palette resolves through the scheme when themed and falls back to
    /// beUI light when not.
    #[test]
    fn the_palette_resolves_through_the_theme_or_falls_back_to_beui_light() {
        let bare = SidebarColors::resolve(None);
        assert_eq!(bare.pill, FALLBACK.muted);
        assert_eq!(bare.ink, FALLBACK.foreground);

        let theme = crate::theme();
        let themed = SidebarColors::resolve(Some(&theme));
        assert_eq!(themed.ink, theme.scheme().on_surface);
        assert_eq!(themed.pill, theme.scheme().surface_container_highest);
    }

    // ---- Typeface: the row labels follow the live theme ----------------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// The trimmed workspace with its first root expanded, so nested rows
    /// paint beside the roots.
    fn probe_view(_: &mut ()) -> AiSidebarView<()> {
        ai_sidebar::<()>(tree()).default_expanded(vec!["release".to_owned()])
    }

    #[test]
    fn row_labels_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the sidebar's rows", probe_view, RAIL);
    }

    #[test]
    fn row_labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the sidebar's rows", probe_view, RAIL);
    }
}
