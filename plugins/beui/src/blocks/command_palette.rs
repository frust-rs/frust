//! Ports beUI's `command-palette` block — `components/motion/command-palette.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `command-palette`: *"⌘K palette with fuzzy filter,
//! spring-animated active row and glass surface."*
//!
//! | upstream | here |
//! |---|---|
//! | panel `w-full max-w-xl rounded-2xl border border-border bg-card shadow-2xl` | [`PALETTE_MAX_WIDTH`], [`PALETTE_RADIUS`], the shared panel chrome |
//! | `PANEL_SPRING` `{stiffness 560, damping 40, mass 0.5}` | [`PALETTE_PANEL_SPRING`] |
//! | panel exit `{duration 0.12, EASE_OUT}` | [`PALETTE_EXIT`] |
//! | backdrop fade `0.18` in / `0.12` out | [`PALETTE_SCRIM_FADE`] (one value — see the degradations) |
//! | input row `flex items-center gap-3 border-b border-border px-4`, field `h-12 text-sm` | [`PALETTE_INPUT_HEIGHT`], [`PALETTE_PADDING_X`], [`PALETTE_GAP`] |
//! | `Search` `h-4 w-4 text-muted-foreground` | [`draw_search`] |
//! | `kbd … px-1.5 py-0.5 text-[10px]` reading `ESC` | [`PALETTE_KBD_LABEL`], [`PALETTE_KBD_PADDING_X`] |
//! | list `max-h-[60vh] overflow-y-auto p-2` | [`PALETTE_MAX_HEIGHT_FRACTION`], [`PALETTE_LIST_PADDING`] |
//! | group heading `px-2 py-1.5 text-[10px] font-semibold uppercase tracking-wider` | [`PALETTE_HEADING_HEIGHT`], [`PALETTE_SMALL_TEXT`] |
//! | row `rounded-md px-2 py-2 text-sm gap-3`, active `text-foreground` else `text-muted-foreground` | [`PALETTE_ROW_HEIGHT`], [`PALETTE_ROW_PADDING_X`] |
//! | active pill `bg-primary/[0.05]` on `{stiffness 480, damping 38}` | [`PALETTE_PILL_ALPHA`], [`PALETTE_PILL_SPRING`] |
//! | empty `p-8 text-center text-sm` | [`PALETTE_EMPTY_PADDING`] |
//! | `fuzzyMatch` — case-folded subsequence over label + group + keywords | [`command_palette_matches`] |
//! | `useRowCursor` — clamped cursor, dropped when the query changes | [`CommandPaletteWidget::active_row`] |
//!
//! # Shape of the port
//!
//! The palette is one widget hosted by [`crate::overlay::modal`] (centred), the
//! same division of labour [`crate::components::center_morph_modal`] documents:
//! the scrim, the barrier, Escape, the backdrop click and the staged exit are
//! the host's, and everything inside the panel is this module's.
//!
//! The **query is controlled** — it arrives as a prop and every edit is reported
//! through `on_query_change` — like every other editable in the catalog. The
//! **highlight is not app data**: it is transient view state the widget owns,
//! exactly as upstream's `useRowCursor` does, reset to the first row whenever
//! the visible row set changes. `on_select` reports the item's index **into the
//! unfiltered `items` vector**, so a caller acts on it without re-running the
//! filter.
//!
//! # The keyboard, and why the field does not own it
//!
//! The panel widget intercepts `ArrowDown`/`ArrowUp`/`Enter` **before** routing
//! to the wrapped field, and forwards everything else (characters, Backspace,
//! within-text arrows). Escape is deliberately neither handled nor forwarded:
//! the baseline field treats it as *blur* and would swallow the very key that
//! closes the palette, so returning `Ignored` lets the enclosing host dismiss
//! and play the exit ramp. That is the same three-key interception
//! `plugins/shadcn/src/components/command.rs` established, and the reason this
//! block does not reuse [`crate::components::combobox`], whose own module docs
//! record that a wrapped field leaves it with no keyboard navigation at all.
//!
//! # The global shortcut is the app's binding
//!
//! Upstream installs a `window` `keydown` listener for ⌘K/Ctrl+K. A widget here
//! has no window-level key seam — a key reaches a widget only down the focus
//! chain — so the shortcut stays the **app's** job and this module exposes
//! [`CommandPaletteController`] for it: a cheap shared flag the app toggles from
//! its own binding and the palette reads on the next rebuild.
//!
//! ```no_run
//! use frust_beui::blocks::command_palette::{CommandPaletteController, command_palette, command_palette_item};
//!
//! struct App {
//!     palette: CommandPaletteController,
//!     query: String,
//! }
//!
//! # fn demo(app: &App) {
//! // The app's own key binding — a root-level key handler, a menu item, a
//! // toolbar button — toggles the controller:
//! app.palette.toggle();
//!
//! // ...and the palette reads it when the view is next built:
//! let _view = command_palette(
//!     vec![command_palette_item("Open file").group("File").hint("⌘O")],
//!     app.query.clone(),
//!     |state: &mut App, text| state.query = text,
//!     |_state: &mut App, index| println!("selected {index}"),
//! )
//! .controller(&app.palette);
//! # }
//! ```
//!
//! # Degradations against the web original
//!
//! - **The panel is centred, not pinned at `18vh`.** Upstream's layer is
//!   `top-[18vh] items-start`; [`crate::overlay::ModalMount`] offers a centred
//!   mount or an edge mount and no top-biased centre. Widening the host is a
//!   change to the shared seam, not to this block.
//! - **One scrim-fade duration, not two.** Upstream fades the backdrop in over
//!   `0.18` and out over `0.12`; [`crate::overlay::ModalConfig::scrim_fade`] is
//!   one value each way, so the entrance figure is kept.
//! - **No backdrop blur.** `backdrop-filter: blur(12px) saturate(140%)` has no
//!   `PaintScene` primitive; the host's dimming scrim carries the separation,
//!   the same call [`crate::components::morphing_modal`] documents.
//! - **No icons and no badges.** `CommandItem.icon` is a lucide component and
//!   `badge` an arbitrary node; the catalog has no icon vocabulary and no
//!   arbitrary-child protocol here, so a row is its label plus its `hint`
//!   ([`CommandPaletteItem::hint`], upstream's trailing `kbd`).
//! - **A long list clips rather than scrolls.** Upstream's list is
//!   `overflow-y-auto`; a scrollable panel would have to mount a
//!   [`frust::scroll_view`] inside a host, which the overlay seam forbids.
//!   [`PALETTE_MAX_HEIGHT_FRACTION`] caps the panel at upstream's `60vh` and the
//!   rows beyond it are clipped — the same cap-and-clip
//!   [`crate::components::combobox`] takes.
//! - **No auto-focus on open.** There is no focus-on-appear hook; the host
//!   claims focus on the first press that lands on it, which is what makes the
//!   arrows, Enter and Escape reachable. Recorded by
//!   [`crate::overlay::anchored`] as the seam-level gap.
//! - **The staggered row entrance is an addition.** Upstream fades the panel's
//!   contents uniformly with the panel. The rows here arrive
//!   [`PALETTE_STAGGER`] apart; `reduce_motion` collapses it back to upstream's
//!   single beat.
//! - **The wrapped field paints its own `surface` fill.** [`frust::text_input`]
//!   has no background override; the strip behind the query row is the theme's
//!   surface rather than the panel's card fill, in a theme where the two differ.
//!   The same wrapping gap [`crate::components::input`] documents.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx,
    NamedKey, PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, SemanticsCtx,
    Size, Vec2, View, Widget, any, build_child, erase_callback, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{FrameTime, SpringDescription, Theme, text_input};

use crate::components::popover::{PanelChrome, lerp, paint_panel, resolve_panel};
use crate::motion::{Ramp, Stagger};
use crate::overlay::{
    ModalConfig, ModalContent, ModalExtent, ModalLimit, ModalView, ModalWidget, PANEL_MARGIN, modal,
};
use crate::press::{Lane, inside, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// `max-w-xl` — the panel's width cap, in logical px.
pub const PALETTE_MAX_WIDTH: f64 = 576.0;

/// `rounded-2xl` — the panel's corner radius, in logical px.
pub const PALETTE_RADIUS: f64 = style::RADIUS_2XL;

/// `max-h-[60vh]` — the share of the host area the whole panel may take.
pub const PALETTE_MAX_HEIGHT_FRACTION: f64 = 0.6;

/// The width the panel falls back to under horizontally-unbounded constraints —
/// a host always bounds it, so this only covers the degenerate mount.
pub const PALETTE_FALLBACK_WIDTH: f64 = 512.0;

/// `h-12` — the query row's height, in logical px.
pub const PALETTE_INPUT_HEIGHT: f64 = 48.0;

/// `px-4` — the query row's horizontal padding, in logical px.
pub const PALETTE_PADDING_X: f64 = 16.0;

/// `gap-3` — the gap between the search mark, the field and the hint, in
/// logical px.
pub const PALETTE_GAP: f64 = 12.0;

/// `p-2` — the list's padding, in logical px.
pub const PALETTE_LIST_PADDING: f64 = 8.0;

/// `px-2 py-2` around a `text-sm` line — one result row's height, in logical px.
pub const PALETTE_ROW_HEIGHT: f64 = 36.0;

/// `px-2` — a row's horizontal padding, in logical px.
pub const PALETTE_ROW_PADDING_X: f64 = 8.0;

/// `rounded-md` — a row's (and the active pill's) corner radius, in logical px.
pub const PALETTE_ROW_RADIUS: f64 = style::RADIUS_MD;

/// A group heading's row height (`py-1.5` around a `text-[10px]` line), in
/// logical px.
pub const PALETTE_HEADING_HEIGHT: f64 = 24.0;

/// `text-[10px]` — the size of a heading, a hint and the ESC key cap, in
/// logical px.
pub const PALETTE_SMALL_TEXT: f64 = 10.0;

/// `p-8` — the empty state's padding, in logical px.
pub const PALETTE_EMPTY_PADDING: f64 = 32.0;

/// The empty state's row height, in logical px.
pub const PALETTE_EMPTY_HEIGHT: f64 = PALETTE_EMPTY_PADDING * 2.0 + style::LINE_HEIGHT_INPUT;

/// `px-1.5` — the horizontal padding inside a key cap, in logical px.
pub const PALETTE_KBD_PADDING_X: f64 = 6.0;

/// `py-0.5` around a `text-[10px]` line — a key cap's height, in logical px.
pub const PALETTE_KBD_HEIGHT: f64 = 18.0;

/// The text on the query row's trailing key cap.
pub const PALETTE_KBD_LABEL: &str = "ESC";

/// `bg-primary/[0.05]` — the active row pill's alpha over the primary role.
pub const PALETTE_PILL_ALPHA: f32 = 0.05;

// ---- Motion ----------------------------------------------------------------

/// `PANEL_SPRING` — the panel's entrance. Authored inline upstream rather than
/// in `lib/ease.ts`, with the stated intent that a palette opened by shortcut
/// "many times a day … must read as instant".
pub const PALETTE_PANEL_SPRING: SpringDescription = SpringDescription {
    mass: 0.5,
    stiffness: 560.0,
    damping: 40.0,
};

/// The active pill's travel spring — upstream's inline
/// `{stiffness: 480, damping: 38}` (Motion's default unit mass), deliberately
/// tighter than [`crate::tokens::motion::SPRING_LAYOUT`] "so it never lags the
/// active row".
pub const PALETTE_PILL_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 480.0,
    damping: 38.0,
};

/// The panel's exit (`{duration: 0.12, ease: EASE_OUT}`).
pub const PALETTE_EXIT: Duration = Duration::from_millis(120);

/// How long the backdrop takes to fade (upstream's `0.18` entrance — see the
/// [module docs](self)' degradations for the missing exit figure).
pub const PALETTE_SCRIM_FADE: Duration = Duration::from_millis(180);

/// How far apart the rows arrive during the entrance. An addition — see the
/// [module docs](self).
pub const PALETTE_STAGGER: Duration = Duration::from_millis(18);

/// How long one row's own entrance takes.
pub const PALETTE_ROW_ENTER: Duration = Duration::from_millis(160);

/// How far below its resting position a row starts, in logical px.
pub const PALETTE_ROW_LIFT: f64 = 6.0;

/// The [`ModalConfig`] a command palette is hosted with: `w-full max-w-xl`
/// inside `inset-4`, capped at [`PALETTE_MAX_HEIGHT_FRACTION`] of the area.
pub fn command_palette_config() -> ModalConfig {
    ModalConfig::centered()
        .width(
            ModalExtent::Fraction(1.0),
            ModalLimit::Px(PALETTE_MAX_WIDTH),
        )
        .height(
            ModalExtent::Hug,
            ModalLimit::Fraction(PALETTE_MAX_HEIGHT_FRACTION),
        )
        .margin(PANEL_MARGIN)
        .scrim_fade(PALETTE_SCRIM_FADE)
        .ramps(
            Ramp::spring(PALETTE_PANEL_SPRING),
            Ramp::eased(PALETTE_EXIT, EASE_OUT),
        )
}

// ---- Items -----------------------------------------------------------------

/// One command in the palette — upstream's `CommandItem`, minus the icon and
/// badge nodes the [module docs](self) record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPaletteItem {
    label: String,
    group: Option<String>,
    hint: Option<String>,
    keywords: Vec<String>,
}

/// A command showing `label`, in the default group.
pub fn command_palette_item(label: impl Into<String>) -> CommandPaletteItem {
    CommandPaletteItem {
        label: label.into(),
        group: None,
        hint: None,
        keywords: Vec::new(),
    }
}

impl CommandPaletteItem {
    /// Put this command under a named heading (`group`). Commands are grouped in
    /// item order; an ungrouped command falls under [`PALETTE_DEFAULT_GROUP`].
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Set the trailing shortcut hint (`hint`, rendered as upstream's `kbd`).
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Add search keywords the filter also matches against (`keywords`).
    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// This command's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The heading this command sits under.
    pub fn group_name(&self) -> &str {
        self.group.as_deref().unwrap_or(PALETTE_DEFAULT_GROUP)
    }
}

/// The heading an ungrouped command falls under (upstream's `it.group ??
/// "Results"`).
pub const PALETTE_DEFAULT_GROUP: &str = "Results";

/// Whether `item` matches `query` — upstream's `fuzzyMatch`, applied to the
/// label, the group name and every keyword.
///
/// The match is a case-folded **subsequence** rather than a substring, so `cmp`
/// finds *Command Palette*. An empty query matches everything.
pub fn command_palette_matches(item: &CommandPaletteItem, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if fuzzy_match(query, &item.label) || fuzzy_match(query, item.group_name()) {
        return true;
    }
    item.keywords.iter().any(|word| fuzzy_match(query, word))
}

/// `fuzzyMatch(needle, hay)`: whether the case-folded `needle` is a subsequence
/// of the case-folded `hay`.
fn fuzzy_match(needle: &str, hay: &str) -> bool {
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    if needle.is_empty() {
        return true;
    }
    let mut at = 0usize;
    for ch in hay.to_lowercase().chars() {
        if ch == needle[at] {
            at += 1;
            if at == needle.len() {
                return true;
            }
        }
    }
    false
}

// ---- The controller --------------------------------------------------------

/// The palette's open flag, shared between an app's own shortcut binding and
/// the view it builds — see the [module docs](self)' shortcut section.
///
/// A cheap `Clone` handle around one `Cell<bool>` (the shape
/// [`crate::overlay::OverlayAnchor`] uses for the same reason): the app toggles
/// it from wherever its binding lives, and [`CommandPaletteView::controller`]
/// reads it on the next rebuild. Dismissal — Escape, a backdrop press, a
/// selection — writes `false` back into it, so the app never has to mirror the
/// flag itself.
#[derive(Clone, Debug, Default)]
pub struct CommandPaletteController(Rc<Cell<bool>>);

impl CommandPaletteController {
    /// A closed controller.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the palette is open.
    pub fn is_open(&self) -> bool {
        self.0.get()
    }

    /// Set the open flag.
    pub fn set_open(&self, open: bool) {
        self.0.set(open);
    }

    /// Open the palette.
    pub fn open(&self) {
        self.set_open(true);
    }

    /// Close the palette.
    pub fn close(&self) {
        self.set_open(false);
    }

    /// Flip the open flag, returning the new value — an app's ⌘K binding in one
    /// call.
    pub fn toggle(&self) -> bool {
        let next = !self.is_open();
        self.set_open(next);
        next
    }
}

// ---- The component ---------------------------------------------------------

/// The mutable configuration the outer builder writes and the inner panel reads
/// — the handle shape [`crate::components::popover`] documents.
type PaletteHandle = Rc<RefCell<PaletteConfig>>;

/// The open-change hook, written after the panel view already exists.
type OpenChangeHandle<State> = Rc<RefCell<Option<Rc<dyn Fn(&mut State, bool)>>>>;

/// A view-held query-edit callback (erased on build).
type OnQueryChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// What the panel needs from its component's builders.
#[derive(Clone, Debug, PartialEq)]
struct PaletteConfig {
    open: bool,
    placeholder: String,
    empty: String,
}

/// A declarative beUI command palette. See [`command_palette`].
pub struct CommandPaletteView<State: 'static> {
    inner: ModalView<State>,
    config: PaletteHandle,
    controller: Rc<RefCell<Option<CommandPaletteController>>>,
    on_open_change: OpenChangeHandle<State>,
}

/// Build a command palette over `items`, filtered by `query`.
///
/// Mount it as the top child of a full-area [`frust::Stack`] with
/// [`CommandPaletteView::open`] (or [`CommandPaletteView::controller`]) carrying
/// the app's own flag, or push it as a transparent navigator page with
/// [`crate::overlay::show_modal`].
///
/// `on_query_change(state, text)` reports each edit of the filter field, and
/// `on_select(state, index)` reports an activation with the item's index into
/// the **unfiltered** `items` vector.
pub fn command_palette<State: 'static, F, G>(
    items: Vec<CommandPaletteItem>,
    query: impl Into<String>,
    on_query_change: F,
    on_select: G,
) -> CommandPaletteView<State>
where
    F: Fn(&mut State, String) + 'static,
    G: Fn(&mut State, usize) + 'static,
{
    let config: PaletteHandle = Rc::new(RefCell::new(PaletteConfig {
        open: true,
        placeholder: "Type a command or search…".to_string(),
        empty: "No results found.".to_string(),
    }));
    let controller: Rc<RefCell<Option<CommandPaletteController>>> = Rc::new(RefCell::new(None));
    let on_open_change: OpenChangeHandle<State> = Rc::new(RefCell::new(None));
    let panel = PalettePanelView {
        items,
        query: query.into(),
        config: config.clone(),
        controller: controller.clone(),
        on_query_change: Rc::new(on_query_change),
        on_select: Rc::new(on_select),
        on_open_change: on_open_change.clone(),
    };
    let dismiss_controller = controller.clone();
    let dismiss_hook = on_open_change.clone();
    CommandPaletteView {
        inner: modal(panel, command_palette_config()).on_dismiss(move |state: &mut State| {
            close_through(&dismiss_controller, &dismiss_hook, state);
        }),
        config,
        controller,
        on_open_change,
    }
}

/// Run the shared close path: clear the controller (when one is installed), then
/// report `false` to the app.
///
/// The hook is cloned **out** of its cell before it runs — an app callback may
/// rebuild the tree, and a borrow held across it would be live during that pass.
fn close_through<State: 'static>(
    controller: &Rc<RefCell<Option<CommandPaletteController>>>,
    hook: &OpenChangeHandle<State>,
    state: &mut State,
) {
    let controller = controller.borrow().clone();
    if let Some(controller) = controller {
        controller.close();
    }
    let hook = hook.borrow().clone();
    if let Some(hook) = hook {
        hook(state, false);
    }
}

impl<State: 'static> CommandPaletteView<State> {
    /// Hand the palette the app's open flag. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Drive the palette from a shared [`CommandPaletteController`] instead of a
    /// plain flag: its current value becomes this frame's open state, and every
    /// dismissal writes `false` back into it.
    ///
    /// Use this **or** [`open`](Self::open), not both — whichever runs last
    /// wins.
    pub fn controller(self, controller: &CommandPaletteController) -> Self {
        *self.controller.borrow_mut() = Some(controller.clone());
        self.open(controller.is_open())
    }

    /// Set the filter field's placeholder.
    pub fn placeholder(self, placeholder: impl Into<String>) -> Self {
        self.config.borrow_mut().placeholder = placeholder.into();
        self
    }

    /// Set the text shown when nothing matches the query.
    pub fn empty(self, empty: impl Into<String>) -> Self {
        self.config.borrow_mut().empty = empty.into();
        self
    }

    /// The accessibility label the panel is announced with.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    /// Set the open-change callback: Escape, a backdrop press, a back press or a
    /// selection all report `false`.
    ///
    /// Upstream closes itself on a selection (`it.onSelect(); setOpen(false)`),
    /// so a selection fires `on_select` **and** this callback, in that order.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(self, on_open_change: F) -> Self {
        *self.on_open_change.borrow_mut() = Some(Rc::new(on_open_change));
        self
    }

    /// Set the exit-settled callback — state-free, fired from the paint that
    /// finishes the panel's exit.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.inner = self.inner.on_close(on_close);
        self
    }
}

impl<State: 'static> View<State> for CommandPaletteView<State> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

impl<State: 'static> ModalContent<State> for CommandPaletteView<State> {
    fn modal_dismissable(&self) -> bool {
        self.inner.config.dismissable
    }
}

// ---- The panel -------------------------------------------------------------

/// The palette's panel: the query row, the grouped rows and the active pill.
struct PalettePanelView<State: 'static> {
    items: Vec<CommandPaletteItem>,
    query: String,
    config: PaletteHandle,
    controller: Rc<RefCell<Option<CommandPaletteController>>>,
    on_query_change: OnQueryChange<State>,
    on_select: OnSelect<State>,
    on_open_change: OpenChangeHandle<State>,
}

/// What a rendered row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    /// A group heading.
    Heading,
    /// A selectable command, carrying its index into the unfiltered items.
    Item(usize),
    /// The empty state.
    Empty,
}

/// One laid-out row.
struct Row {
    kind: RowKind,
    label: LabelRun,
    hint: Option<LabelRun>,
    /// The row's box in the panel's own space.
    rect: Rect,
}

/// The retained widget for a command palette's panel.
pub struct CommandPaletteWidget {
    /// The wrapped filter field.
    field: ChildPod,
    items: Vec<CommandPaletteItem>,
    query: String,
    rows: Vec<Row>,
    config: PaletteConfig,
    /// The `ESC` key cap on the query row.
    kbd: LabelRun,
    /// The highlighted row, as an index into [`rows`](Self::rows).
    active: Option<usize>,
    /// The row a primary `Down` armed.
    armed: Option<usize>,
    /// The pill's travel between rows, `0.0` at `pill_from`, `1.0` at the active
    /// row.
    pill: Lane,
    pill_from: Rect,
    pill_to: Rect,
    /// The pill's own opacity, kept apart from its travel so a row-to-row move
    /// never restarts the fade.
    pill_alpha: Lane,
    /// When the current entrance started, for the row stagger.
    entered_at: Option<FrameTime>,
    /// The query row's own bottom edge, so the event pass can tell the field's
    /// band from the list's.
    list_top: f64,
    on_query_change: ErasedArgCallback<String>,
    on_select: ErasedArgCallback<usize>,
    on_close: ErasedCallback,
}

impl CommandPaletteWidget {
    /// The highlighted row, as an index into the rendered rows.
    pub fn active_row(&self) -> Option<usize> {
        self.active
    }

    /// The item index the highlighted row carries, into the unfiltered items.
    pub fn active_item(&self) -> Option<usize> {
        match self.rows.get(self.active?)?.kind {
            RowKind::Item(index) => Some(index),
            _ => None,
        }
    }

    /// How many rows — headings and the empty state included — are rendered for
    /// the current query.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// One row's box in the panel's own space.
    pub fn row_rect(&self, index: usize) -> Option<Rect> {
        self.rows.get(index).map(|row| row.rect)
    }

    /// The rows the current `items`/`query` pair renders to: a heading opens
    /// each group, in item order, and the empty state stands in for none.
    fn build_rows(items: &[CommandPaletteItem], query: &str, empty: &str) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut group: Option<&str> = None;
        for (index, item) in items.iter().enumerate() {
            if !command_palette_matches(item, query) {
                continue;
            }
            let name = item.group_name();
            if group != Some(name) {
                rows.push(Row::new(RowKind::Heading, name, None));
                group = Some(name);
            }
            rows.push(Row::new(
                RowKind::Item(index),
                &item.label,
                item.hint.as_deref(),
            ));
        }
        if rows.is_empty() {
            rows.push(Row::new(RowKind::Empty, empty, None));
        }
        rows
    }

    /// The first selectable row, if the current rows have one.
    fn first_item(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row.kind, RowKind::Item(_)))
    }

    /// Adopt a freshly-built row set, returning the flags it costs.
    ///
    /// The highlight returns to the first row, which is where upstream's
    /// query-stamped cursor also lands once it is dropped.
    fn adopt_rows(&mut self, rows: Vec<Row>) -> ChangeFlags {
        self.rows = rows;
        self.armed = None;
        let first = self.first_item();
        self.active = first;
        self.pill = Lane::at_rest(Ramp::spring(PALETTE_PILL_SPRING), 1.0);
        self.pill_from = Rect::ZERO;
        self.pill_to = Rect::ZERO;
        self.pill_alpha = Lane::at_rest(
            Ramp::spring(PALETTE_PILL_SPRING),
            if first.is_some() { 1.0 } else { 0.0 },
        );
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }

    /// Move the pill onto `next`, springing from wherever it is now.
    fn set_active(&mut self, next: Option<usize>) -> bool {
        if self.active == next {
            return false;
        }
        let current = self.pill_rect();
        self.active = next;
        if let Some(row) = next.and_then(|i| self.rows.get(i)) {
            // A pill arriving from nowhere starts on its own row rather than
            // sliding in from the panel origin.
            self.pill_from = if self.pill_alpha.target() == 0.0 && current.is_zero_area() {
                row.rect
            } else {
                current
            };
            self.pill_to = row.rect;
            self.pill = Lane::at_rest(Ramp::spring(PALETTE_PILL_SPRING), 0.0);
            self.pill.retarget(1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(PALETTE_PILL_SPRING), 1.0);
        } else {
            self.pill_from = current;
            self.pill_to = current;
            self.pill = Lane::at_rest(Ramp::spring(PALETTE_PILL_SPRING), 1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(PALETTE_PILL_SPRING), 0.0);
        }
        true
    }

    /// The pill's rect right now — its resting row's box once the travel has
    /// settled.
    fn pill_rect(&self) -> Rect {
        if self.pill_from.is_zero_area() && self.pill_to.is_zero_area() {
            return self
                .active
                .and_then(|i| self.rows.get(i))
                .map_or(Rect::ZERO, |row| row.rect);
        }
        let t = self.pill.value().clamp(0.0, 1.0);
        Rect::new(
            lerp(self.pill_from.x0, self.pill_to.x0, t),
            lerp(self.pill_from.y0, self.pill_to.y0, t),
            lerp(self.pill_from.x1, self.pill_to.x1, t),
            lerp(self.pill_from.y1, self.pill_to.y1, t),
        )
    }

    /// The next selectable row in `step`'s direction, **clamped** at both ends —
    /// upstream's `useRowCursor.moveActive`, which is
    /// `Math.min(Math.max(at + direction, 0), last)` and so never wraps.
    fn step_active(&self, step: isize) -> Option<usize> {
        let items: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row.kind, RowKind::Item(_)))
            .map(|(i, _)| i)
            .collect();
        if items.is_empty() {
            return None;
        }
        let at = self
            .active
            .and_then(|row| items.iter().position(|i| *i == row))
            .unwrap_or(0) as isize;
        let last = items.len() as isize - 1;
        Some(items[at.saturating_add(step).clamp(0, last) as usize])
    }

    /// The selectable row `position` (in the panel's own space) lands on.
    fn row_at(&self, position: Point) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row.kind, RowKind::Item(_)) && row.rect.contains(position))
    }

    /// The stagger the rows arrive on.
    fn stagger(reduce: bool) -> Stagger {
        let base = Stagger::eased(PALETTE_STAGGER, PALETTE_ROW_ENTER, EASE_OUT);
        if reduce { base.collapsed() } else { base }
    }

    /// Report `row`'s activation, then close — upstream's
    /// `it.onSelect(); setOpen(false)`.
    fn select(&mut self, ctx: &mut EventCtx, row: usize) {
        let Some(RowKind::Item(index)) = self.rows.get(row).map(|row| row.kind) else {
            return;
        };
        (self.on_select)(ctx, index);
        (self.on_close)(ctx);
    }
}

impl Row {
    fn new(kind: RowKind, label: &str, hint: Option<&str>) -> Self {
        Row {
            kind,
            label: LabelRun::new(label.to_string()),
            hint: hint.map(LabelRun::new),
            rect: Rect::ZERO,
        }
    }

    /// This row's own height, in logical px.
    fn height(&self) -> f64 {
        match self.kind {
            RowKind::Heading => PALETTE_HEADING_HEIGHT,
            RowKind::Item(_) => PALETTE_ROW_HEIGHT,
            RowKind::Empty => PALETTE_EMPTY_HEIGHT,
        }
    }
}

impl<State: 'static> PalettePanelView<State> {
    /// The wrapped baseline filter field, with its own chrome suppressed (this
    /// widget paints the row's rule, mark and key cap).
    fn field(&self) -> AnyView<State> {
        let on_change = self.on_query_change.clone();
        any(
            text_input(self.query.clone(), move |state: &mut State, text| {
                on_change(state, text)
            })
            .placeholder(self.config.borrow().placeholder.clone())
            .padding(0.0, 0.0)
            .border_width(0.0)
            .corner_radius(0.0)
            .focus_ring_width(0.0),
        )
    }

    /// The shared close path, as a view-held callback.
    fn close_callback(&self) -> Rc<dyn Fn(&mut State)> {
        let controller = self.controller.clone();
        let hook = self.on_open_change.clone();
        Rc::new(move |state: &mut State| close_through(&controller, &hook, state))
    }
}

impl<State: 'static> View<State> for PalettePanelView<State> {
    type Element = CommandPaletteWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CommandPaletteWidget {
        let config = self.config.borrow().clone();
        let rows = CommandPaletteWidget::build_rows(&self.items, &self.query, &config.empty);
        let first = rows
            .iter()
            .position(|row| matches!(row.kind, RowKind::Item(_)));
        CommandPaletteWidget {
            field: build_child(&self.field(), ctx),
            items: self.items.clone(),
            query: self.query.clone(),
            rows,
            config,
            kbd: LabelRun::new(PALETTE_KBD_LABEL),
            active: first,
            armed: None,
            pill: Lane::at_rest(Ramp::spring(PALETTE_PILL_SPRING), 1.0),
            pill_from: Rect::ZERO,
            pill_to: Rect::ZERO,
            pill_alpha: Lane::at_rest(
                Ramp::spring(PALETTE_PILL_SPRING),
                if first.is_some() { 1.0 } else { 0.0 },
            ),
            entered_at: None,
            list_top: PALETTE_INPUT_HEIGHT,
            on_query_change: erase_callback_arg(&self.on_query_change),
            on_select: erase_callback_arg(&self.on_select),
            on_close: erase_callback(&self.close_callback()),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CommandPaletteWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.field(), &self.field(), &mut element.field, ctx);
        let config = self.config.borrow().clone();
        let reopened = config.open && !element.config.open;
        if element.config != config {
            element.config = config.clone();
            flags |= ChangeFlags::PAINT;
        }
        // The row set is a pure function of the items, the query and the empty
        // text: rebuilt only when one of the three actually changed, so a
        // per-frame rebuild never restarts the pill or drops the highlight.
        if element.items != self.items || element.query != self.query {
            element.items = self.items.clone();
            element.query = self.query.clone();
            let rows =
                CommandPaletteWidget::build_rows(&element.items, &element.query, &config.empty);
            flags |= element.adopt_rows(rows);
        }
        if reopened {
            // A fresh open episode: the stagger is timed from the frame the
            // panel next paints, and the highlight returns to the first row.
            element.entered_at = None;
            element.armed = None;
            let first = element.first_item();
            element.active = first;
            flags |= ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_query_change = erase_callback_arg(&self.on_query_change);
        element.on_select = erase_callback_arg(&self.on_select);
        element.on_close = erase_callback(&self.close_callback());
        flags
    }

    fn teardown(&self, element: &mut CommandPaletteWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.field(), &mut element.field, ctx);
    }
}

// ---- Text styles -----------------------------------------------------------

/// A result row's label style (`text-sm`).
fn row_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        ..TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK)
    }
}

/// A hint's / key cap's style (`text-[10px] font-medium`).
fn small_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_small.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(PALETTE_SMALL_TEXT as f32, crate::text::SHAPING_INK)
    }
}

/// A group heading's style (`text-[10px] font-semibold`).
fn heading_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..small_style(theme)
    }
}

/// Paint lucide's `search` mark — a ring plus its handle — centred on `centre`,
/// `extent` logical px on a side.
///
/// Public, and shared with [`crate::blocks::morphing_search`], which leads its
/// own query row with the same mark: one search glyph per catalog, drawn where
/// the first component that needed it put it — the same arrangement
/// [`crate::components::context_menu`] has with `popover`'s panel helpers.
pub fn draw_search(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    // `circle cx=11 cy=11 r=8` plus `path m21 21-4.35-4.35`, taken relative to
    // the 24-unit box's own centre.
    let ring = kurbo::Circle::new(Point::new(-scale, -scale), 8.0 * scale);
    let mut path: BezPath = kurbo::Shape::to_path(&ring, style::PATH_TOLERANCE);
    path.move_to(Point::new(9.0 * scale, 9.0 * scale));
    path.line_to(Point::new(4.65 * scale, 4.65 * scale));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// lucide's own viewBox extent.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// lucide's default `strokeWidth`, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

impl Widget for CommandPaletteWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let row_style = row_style(theme);
        let small_style = small_style(theme);
        let heading_style = heading_style(theme);

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            PALETTE_FALLBACK_WIDTH
        };

        // The query row: the search mark leads, the key cap trails, the field
        // takes what is left.
        let kbd_width = self.kbd.layout(ctx, &small_style).width + PALETTE_KBD_PADDING_X * 2.0;
        let lead = PALETTE_PADDING_X + style::ICON_SIZE + PALETTE_GAP;
        let field_width = (width - lead - kbd_width - PALETTE_GAP - PALETTE_PADDING_X).max(0.0);
        self.field.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(field_width, PALETTE_INPUT_HEIGHT)),
        );
        self.field.set_origin(Point::new(lead, 0.0));
        self.list_top = PALETTE_INPUT_HEIGHT + style::BORDER_WIDTH;

        // The rows, stacked inside the list's padding.
        let mut y = self.list_top + PALETTE_LIST_PADDING;
        for row in &mut self.rows {
            let style = match row.kind {
                RowKind::Heading => &heading_style,
                RowKind::Item(_) | RowKind::Empty => &row_style,
            };
            row.label.layout(ctx, style);
            if let Some(hint) = &mut row.hint {
                hint.layout(ctx, &small_style);
            }
            let height = row.height();
            row.rect = Rect::new(
                PALETTE_LIST_PADDING,
                y,
                width - PALETTE_LIST_PADDING,
                y + height,
            );
            y += height;
        }
        // A pill placed before the first layout has zero-area endpoints; seat it
        // on its row now that the rows have boxes.
        if self.pill_from.is_zero_area() && self.pill_to.is_zero_area() {
            let seat = self.pill_rect();
            self.pill_from = seat;
            self.pill_to = seat;
        }
        bc.constrain(Size::new(width, y + PALETTE_LIST_PADDING))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let accent = theme.map_or(crate::BEUI_LIGHT.primary, |t| t.scheme().primary);
        let cap_fill = theme.map_or(crate::BEUI_LIGHT.background, |t| t.scheme().surface);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let panel = Rect::from_origin_size(Point::ORIGIN, ctx.size());

        paint_panel(scene, origin, panel, PALETTE_RADIUS, chrome);
        scene.push_clip_rounded(origin, ctx.size(), PALETTE_RADIUS);

        // The query row: mark, field, key cap, and the rule under them.
        draw_search(
            scene,
            origin
                + Vec2::new(
                    PALETTE_PADDING_X + style::ICON_SIZE / 2.0,
                    PALETTE_INPUT_HEIGHT / 2.0,
                ),
            style::ICON_SIZE,
            chrome.dim_ink,
        );
        self.field.paint_child(ctx, scene);
        let cap_width = self.kbd.size().width + PALETTE_KBD_PADDING_X * 2.0;
        let cap = Rect::new(
            ctx.size().width - PALETTE_PADDING_X - cap_width,
            (PALETTE_INPUT_HEIGHT - PALETTE_KBD_HEIGHT) / 2.0,
            ctx.size().width - PALETTE_PADDING_X,
            (PALETTE_INPUT_HEIGHT + PALETTE_KBD_HEIGHT) / 2.0,
        );
        scene.fill_rounded_rect(
            origin + cap.origin().to_vec2(),
            cap.size(),
            style::RADIUS_SM,
            cap_fill,
        );
        crate::components::popover::paint_panel_hairline(
            scene,
            origin + cap.origin().to_vec2(),
            cap.size(),
            style::RADIUS_SM,
            chrome.border,
        );
        let cap_text = self.kbd.size();
        self.kbd.paint(
            origin
                + Vec2::new(
                    cap.x0 + PALETTE_KBD_PADDING_X,
                    cap.y0 + (cap.height() - cap_text.height) / 2.0,
                ),
            chrome.dim_ink,
            scene,
        );
        scene.fill_rect(
            origin + Vec2::new(0.0, PALETTE_INPUT_HEIGHT),
            Size::new(ctx.size().width, style::BORDER_WIDTH),
            chrome.border,
        );

        self.paint_rows(ctx, scene, chrome, accent, now, reduce);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.field.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if !self.config.open {
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            return self.handle_key(ctx, event, key);
        }
        // The IME session and the clipboard verbs an `EditCommand` carries both
        // belong to the wrapped field outright — `Key` is handled above,
        // since the palette's own navigation keys intercept before falling
        // through to `handle_key`'s own field forward. Branch on the shared
        // predicate rather than enumerating `Ime`/`EditCommand` separately.
        if event.is_focus_routed() {
            return route_event_single(&mut self.field, ctx, event);
        }
        match event {
            InputEvent::Pointer(p) => self.handle_pointer(ctx, event, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !self.config.open {
            return;
        }
        // The field contributes its own `TextInput` node.
        self.field.semantics_child(ctx);
        let active = self.active;
        ctx.push_container(
            Role::ListBox,
            |_| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    if !matches!(row.kind, RowKind::Item(_)) {
                        continue;
                    }
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(row.label.content());
                        node.set_selected(active == Some(index));
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(field);
}

impl CommandPaletteWidget {
    /// The `Widget::event` key arm: three intercepted keys, Escape left for the
    /// host, everything else the field's — see the [module docs](self).
    fn handle_key(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        key: &KeyEvent,
    ) -> EventResult {
        match &key.key {
            // Neither handled nor forwarded: the field reads Escape as a blur,
            // and the host must see it to dismiss.
            Key::Named(NamedKey::Escape) => EventResult::Ignored,
            Key::Named(NamedKey::ArrowDown) => {
                if self.set_active(self.step_active(1)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.set_active(self.step_active(-1)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::Enter) => {
                let Some(row) = self.active else {
                    return EventResult::Ignored;
                };
                self.select(ctx, row);
                EventResult::Handled
            }
            _ => route_event_single(&mut self.field, ctx, event),
        }
    }

    /// The `Widget::event` pointer arm: the query row is the field's, the list
    /// is this widget's.
    fn handle_pointer(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
    ) -> EventResult {
        // Only a primary press operates the list. `Move` still passes so hover
        // keeps working, and refusing here keeps a secondary press off the
        // field too.
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Ignored;
        }
        if p.position.y < self.list_top || self.field.is_active() {
            return route_event_single(&mut self.field, ctx, event);
        }
        if !inside(p.position, ctx.size()) {
            return EventResult::Ignored;
        }
        let row = self.row_at(p.position);
        match p.phase {
            PointerPhase::Move => {
                // Claimed after the field routing above (the claim-ordering
                // rule).
                if row.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if row.is_some() && self.set_active(row) {
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                let Some(row) = row else {
                    return EventResult::Ignored;
                };
                // Focus is what routes the arrows and Enter here afterwards.
                ctx.request_focus();
                ctx.capture_pointer();
                self.armed = Some(row);
                if self.set_active(Some(row)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if row == Some(armed) {
                    self.select(ctx, armed);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state — it only drops the arm.
            PointerPhase::Cancel => {
                if self.armed.take().is_none() {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
        }
    }

    /// Paint the active pill and every row, each on its own stagger slot.
    fn paint_rows(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        chrome: PanelChrome,
        accent: Color,
        now: FrameTime,
        reduce: bool,
    ) {
        let origin = ctx.origin();
        let travelled = self.pill.advance(now);
        let faded = self.pill_alpha.advance(now);
        if travelled || faded {
            ctx.request_frame();
        }

        let entered = *self.entered_at.get_or_insert(now);
        let elapsed = now.saturating_sub(entered);
        let stagger = Self::stagger(reduce);
        let count = self.rows.len();
        if !stagger.is_settled(elapsed, count) {
            ctx.request_frame();
        }

        // The shared active pill, under the rows — one `layoutId` rect.
        let alpha = self.pill_alpha.value().clamp(0.0, 1.0) as f32;
        if alpha > 0.0 {
            let pill = self.pill_rect();
            if pill.width() > 0.0 && pill.height() > 0.0 {
                scene.fill_rounded_rect(
                    origin + pill.origin().to_vec2(),
                    pill.size(),
                    PALETTE_ROW_RADIUS,
                    style::with_alpha(accent, PALETTE_PILL_ALPHA * alpha),
                );
            }
        }

        for (index, row) in self.rows.iter().enumerate() {
            let revealed = stagger.revealed(elapsed, index, count);
            if revealed <= 0.0 {
                continue;
            }
            let lift = Vec2::new(0.0, (1.0 - revealed) * PALETTE_ROW_LIFT);
            let layered = revealed < 1.0;
            if layered {
                scene.push_layer(
                    origin + row.rect.origin().to_vec2() + lift,
                    row.rect.size(),
                    revealed as f32,
                );
            }
            row.paint(origin + lift, chrome, self.active == Some(index), scene);
            if layered {
                scene.pop_layer();
            }
        }
    }
}

impl Row {
    /// Paint this row's heading, empty text, or label/hint pair.
    fn paint(&self, origin: Point, chrome: PanelChrome, active: bool, scene: &mut dyn PaintScene) {
        let text = self.label.size();
        let centred_y = self.rect.y0 + (self.rect.height() - text.height) / 2.0;
        match self.kind {
            RowKind::Heading => {
                self.label.paint(
                    origin + Vec2::new(self.rect.x0 + PALETTE_ROW_PADDING_X, centred_y),
                    chrome.dim_ink,
                    scene,
                );
            }
            RowKind::Empty => {
                // `text-center`: the catalog shapes its own runs, so the empty
                // line is centred here rather than left to a layout attribute
                // the authoring seam does not carry.
                let x = self.rect.x0 + (self.rect.width() - text.width) / 2.0;
                self.label
                    .paint(origin + Vec2::new(x, centred_y), chrome.dim_ink, scene);
            }
            RowKind::Item(_) => {
                let ink = if active { chrome.ink } else { chrome.dim_ink };
                self.label.paint(
                    origin + Vec2::new(self.rect.x0 + PALETTE_ROW_PADDING_X, centred_y),
                    ink,
                    scene,
                );
                if let Some(hint) = &self.hint {
                    let size = hint.size();
                    hint.paint(
                        origin
                            + Vec2::new(
                                self.rect.x1 - PALETTE_ROW_PADDING_X - size.width,
                                self.rect.y0 + (self.rect.height() - size.height) / 2.0,
                            ),
                        chrome.dim_ink,
                        scene,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, escape, ft_ms, light, pointer, reduced};
    use frust::authoring::text::TextContext;
    use frust::authoring::{EditCommand, Key, KeyEvent, Modifiers, NamedKey};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A window big enough for the palette's `max-w-xl` panel to be capped by
    /// its own width rather than by the area.
    const WINDOW: Size = Size::new(800.0, 600.0);

    #[derive(Default)]
    struct App {
        query: String,
        opens: Vec<bool>,
        selected: Vec<usize>,
    }

    fn items() -> Vec<CommandPaletteItem> {
        vec![
            command_palette_item("Open File")
                .group("File")
                .hint("⌘O")
                .keywords(["load"]),
            command_palette_item("Save File").group("File"),
            command_palette_item("Toggle Theme").group("View"),
            command_palette_item("Zoom In").group("View"),
        ]
    }

    /// The rows an empty query renders: a heading per group, then its commands.
    const ROWS_UNFILTERED: usize = 6;

    /// The palette's own paint-driven clock. Every driver in this catalog is
    /// timed from `PaintCtx::frame_time`, so a harness that paints twice at the
    /// same timestamp reads a spring that has not moved — the clock only ever
    /// goes forward here.
    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        controller: CommandPaletteController,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::themed(light())
        }

        fn themed(theme: Theme) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                tcx: TextContext::new(),
                controller: CommandPaletteController::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(theme));
            h.step(0.0);
            h
        }

        /// Advance the clock by `ms` and run a whole frame: rebuild, layout,
        /// paint.
        fn step(&mut self, ms: f64) {
            self.clock += ms;
            let controller = self.controller.clone();
            let mut logic = move |s: &mut App| {
                frust::Stack(vec![any(command_palette(
                    items(),
                    s.query.clone(),
                    |s: &mut App, text| s.query = text,
                    |s: &mut App, index| s.selected.push(index),
                )
                .controller(&controller)
                .label("Command palette")
                .on_open_change(|s: &mut App, open| s.opens.push(open)))])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        /// Advance by `ms` and paint alone, recording what was drawn.
        fn read_after(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(self.clock));
            rec
        }

        /// Paint far enough ahead that every spring in flight has settled.
        fn read(&mut self) -> Recorder {
            self.read_after(SETTLE_MS)
        }

        /// A whole frame far enough ahead to settle everything.
        fn settle(&mut self) {
            self.step(SETTLE_MS);
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// Open the palette and settle its entrance.
        fn open(&mut self) {
            self.controller.open();
            self.step(0.0);
            self.step(16.0);
            self.settle();
        }

        /// The settled panel's box in window space.
        fn panel(&mut self) -> Rect {
            let rec = self.read();
            let (origin, size, _, _) = *rec
                .rrects
                .iter()
                .find(|(_, _, r, _)| *r == PALETTE_RADIUS)
                .expect("the settled panel");
            Rect::from_origin_size(origin, size)
        }

        /// The active pill's box in window space, once its travel has settled.
        fn pill(&mut self) -> Rect {
            let rec = self.read();
            let (origin, size, _, _) = *rec
                .rrects
                .iter()
                .find(|(_, _, r, _)| *r == PALETTE_ROW_RADIUS)
                .expect("the active pill");
            Rect::from_origin_size(origin, size)
        }

        /// The top of the first command row (the "File" heading sits above it),
        /// in window space.
        fn first_row_top(&mut self) -> f64 {
            self.panel().y0
                + PALETTE_INPUT_HEIGHT
                + style::BORDER_WIDTH
                + PALETTE_LIST_PADDING
                + PALETTE_HEADING_HEIGHT
        }

        /// Press and release inside the query row, which is what puts the
        /// palette on the focus chain (there is no focus-on-appear hook).
        fn focus_query(&mut self) {
            let panel = self.panel();
            let at = panel.origin() + Vec2::new(200.0, PALETTE_INPUT_HEIGHT / 2.0);
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.settle();
        }

        fn key(&mut self, key: NamedKey) {
            self.event(InputEvent::Key(KeyEvent {
                key: Key::Named(key),
                modifiers: Modifiers::default(),
                repeat: false,
            }));
            self.settle();
        }

        /// Type `text` into the filter field the way the app would: the widget
        /// is controlled, so the harness feeds the reported edit straight back.
        fn type_query(&mut self, text: &str) {
            self.state.query = text.to_string();
            self.settle();
        }
    }

    /// Long enough for every ramp in this module — the entrance spring, the
    /// pill spring, the row stagger — to have settled.
    const SETTLE_MS: f64 = 3_000.0;

    /// The height a row set of the given shape lays out to.
    fn expected_height(headings: usize, rows: usize) -> f64 {
        PALETTE_INPUT_HEIGHT
            + style::BORDER_WIDTH
            + PALETTE_LIST_PADDING * 2.0
            + headings as f64 * PALETTE_HEADING_HEIGHT
            + rows as f64 * PALETTE_ROW_HEIGHT
    }

    // ---- The filter -------------------------------------------------------

    #[test]
    fn the_filter_is_a_case_folded_subsequence_over_label_group_and_keywords() {
        let item = command_palette_item("Open File")
            .group("Workspace")
            .keywords(["reveal"]);
        assert!(
            command_palette_matches(&item, ""),
            "empty matches everything"
        );
        assert!(
            command_palette_matches(&item, "opf"),
            "subsequence of the label"
        );
        assert!(command_palette_matches(&item, "OPEN"), "case-folded");
        assert!(command_palette_matches(&item, "wksp"), "matches the group");
        assert!(command_palette_matches(&item, "rvl"), "matches a keyword");
        assert!(
            !command_palette_matches(&item, "fo"),
            "a subsequence is ordered: 'f' before 'o' does not match 'Open File'"
        );
        assert!(!command_palette_matches(&item, "zebra"));
    }

    #[test]
    fn an_ungrouped_command_falls_under_the_default_heading() {
        let item = command_palette_item("Reload");
        assert_eq!(item.group_name(), PALETTE_DEFAULT_GROUP);
        assert_eq!(item.label(), "Reload");
    }

    // ---- The panel and its rows -------------------------------------------

    #[test]
    fn the_open_panel_is_capped_at_the_upstream_width_and_stacks_every_row() {
        let mut h = Harness::new();
        h.open();
        let panel = h.panel();
        assert_eq!(panel.width(), PALETTE_MAX_WIDTH, "`max-w-xl`");
        assert_eq!(panel.height(), expected_height(2, 4));
        assert_eq!(ROWS_UNFILTERED, 6);
    }

    #[test]
    fn a_narrowing_query_drops_the_rows_it_excludes_and_shrinks_the_panel() {
        let mut h = Harness::new();
        h.open();
        let full = h.panel().height();
        // A subsequence of "Zoom In" alone — one heading plus one row.
        h.type_query("zo");
        let narrowed = h.panel().height();
        assert!(narrowed < full, "the panel shrank: {narrowed} < {full}");
        assert_eq!(narrowed, expected_height(1, 1));
    }

    #[test]
    fn a_query_matching_nothing_shows_the_empty_state_alone() {
        let mut h = Harness::new();
        h.open();
        h.type_query("qqqq");
        assert_eq!(
            h.panel().height(),
            PALETTE_INPUT_HEIGHT
                + style::BORDER_WIDTH
                + PALETTE_LIST_PADDING * 2.0
                + PALETTE_EMPTY_HEIGHT
        );
        // ...and no pill, since there is nothing selectable to highlight.
        let rec = h.read();
        assert!(
            !rec.rrects
                .iter()
                .any(|(_, _, r, _)| *r == PALETTE_ROW_RADIUS),
            "an empty result set paints no active pill"
        );
    }

    // ---- Keyboard navigation ----------------------------------------------

    #[test]
    fn the_first_command_is_highlighted_and_the_pill_sits_on_it() {
        let mut h = Harness::new();
        h.open();
        // Row 0 is the "File" heading; row 1 is the first command.
        let first_row_top = h.first_row_top();
        let pill = h.pill();
        assert!((pill.y0 - first_row_top).abs() < 0.001, "{pill:?}");
        assert_eq!(pill.height(), PALETTE_ROW_HEIGHT);
    }

    #[test]
    fn arrow_down_walks_the_commands_and_skips_the_group_headings() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        let first = h.pill().y0;
        h.key(NamedKey::ArrowDown);
        let second = h.pill().y0;
        assert!(
            (second - first - PALETTE_ROW_HEIGHT).abs() < 0.001,
            "one row down: {first} -> {second}"
        );
        // The third command sits past the "View" heading, so the next step is a
        // row plus a heading rather than a bare row.
        h.key(NamedKey::ArrowDown);
        let third = h.pill().y0;
        assert!(
            (third - second - PALETTE_ROW_HEIGHT - PALETTE_HEADING_HEIGHT).abs() < 0.001,
            "the heading is stepped over: {second} -> {third}"
        );
    }

    #[test]
    fn the_arrows_clamp_at_both_ends_rather_than_wrapping() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        let first = h.pill().y0;
        for _ in 0..8 {
            h.key(NamedKey::ArrowDown);
        }
        let last = h.pill().y0;
        assert!(
            last > first,
            "the walk actually moved before it clamped: {first} -> {last}"
        );
        h.key(NamedKey::ArrowDown);
        assert_eq!(
            h.pill().y0,
            last,
            "the last command clamps, it does not wrap"
        );
        for _ in 0..8 {
            h.key(NamedKey::ArrowUp);
        }
        assert!((h.pill().y0 - first).abs() < 0.001, "and so does the first");
    }

    #[test]
    fn a_narrowed_list_returns_the_highlight_to_its_first_row() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        h.key(NamedKey::ArrowDown);
        h.key(NamedKey::ArrowDown);
        // Narrowing to a single command drops the cursor the old rows carried.
        h.type_query("zo");
        let only_row_top = h.first_row_top();
        assert!((h.pill().y0 - only_row_top).abs() < 0.001);
        // ...and Enter commits that row, not the one the old cursor was on.
        h.key(NamedKey::Enter);
        assert_eq!(h.state.selected, vec![3], "\"Zoom In\" is item 3");
    }

    #[test]
    fn enter_selects_the_highlighted_command_and_closes_the_palette() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        h.key(NamedKey::ArrowDown);
        h.key(NamedKey::Enter);
        assert_eq!(h.state.selected, vec![1], "\"Save File\" is item 1");
        assert_eq!(
            h.state.opens.last(),
            Some(&false),
            "upstream closes itself on a selection"
        );
        assert!(!h.controller.is_open(), "and the controller follows");
    }

    /// `EditCommand::Paste` is focus-routed exactly like `Key`/`Ime`: a
    /// clipboard paste dispatched at the palette while the field holds focus
    /// must reach it, closing the gap where a `Ctrl+V` chord's
    /// `ctx.request_paste()` succeeds but the shell's separate top-level
    /// `EditCommand::Paste(text)` dispatch it triggers is then swallowed here.
    #[test]
    fn a_paste_edit_command_reaches_the_focused_field() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        h.event(InputEvent::EditCommand(EditCommand::Paste(
            "zoom".to_string(),
        )));
        h.settle();
        assert_eq!(
            h.state.query, "zoom",
            "the pasted text must reach the focused field"
        );
    }

    /// Guard against over-forwarding: the palette's own navigation keys must
    /// still be intercepted before anything reaches the field, even while the
    /// field holds focus — `is_focus_routed()` only widens the catch-all arm
    /// *after* `handle_key`'s own interception, it must never bypass it.
    #[test]
    fn arrow_down_moves_the_highlight_not_the_field_while_focused() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        h.key(NamedKey::ArrowDown);
        assert_eq!(
            h.state.query, "",
            "ArrowDown must not reach the focused field as typed text"
        );
    }

    #[test]
    fn escape_closes_the_palette_and_the_panel_plays_its_exit() {
        let mut h = Harness::new();
        h.open();
        h.focus_query();
        h.event(escape());
        assert_eq!(h.state.opens, vec![false]);
        assert!(!h.controller.is_open());

        // Still painted through the exit ramp, gone once it has settled — the
        // kept-mounted contract the host stages.
        h.step(0.0);
        let mid = h.read_after(PALETTE_EXIT.as_secs_f64() * 500.0);
        assert!(
            mid.rrects.iter().any(|(_, _, r, _)| *r == PALETTE_RADIUS),
            "the panel is still on screen halfway through its exit"
        );
        let after = h.read_after(PALETTE_EXIT.as_secs_f64() * 1_000.0);
        assert!(
            !after.rrects.iter().any(|(_, _, r, _)| *r == PALETTE_RADIUS),
            "and gone once the exit has settled"
        );
    }

    // ---- The pointer ------------------------------------------------------

    #[test]
    fn a_click_on_a_row_selects_it_and_a_press_that_moves_off_it_does_not() {
        let mut h = Harness::new();
        h.open();
        let panel = h.panel();
        let row_top = h.first_row_top();
        let on_first = Point::new(panel.center().x, row_top + PALETTE_ROW_HEIGHT / 2.0);
        let on_second = Point::new(on_first.x, on_first.y + PALETTE_ROW_HEIGHT);

        // A press that lands on one row and releases over another commits
        // nothing — the armed-and-re-hit rule.
        h.event(pointer(PointerPhase::Down, on_first.x, on_first.y));
        h.event(pointer(PointerPhase::Up, on_second.x, on_second.y));
        assert!(h.state.selected.is_empty());

        h.event(pointer(PointerPhase::Down, on_second.x, on_second.y));
        h.event(pointer(PointerPhase::Up, on_second.x, on_second.y));
        assert_eq!(h.state.selected, vec![1]);
        assert_eq!(h.state.opens.last(), Some(&false));
    }

    #[test]
    fn a_hover_moves_the_highlight_onto_the_row_under_the_pointer() {
        let mut h = Harness::new();
        h.open();
        let panel = h.panel();
        let row_top = h.first_row_top();
        let second = row_top + PALETTE_ROW_HEIGHT * 1.5;
        h.event(pointer(PointerPhase::Move, panel.center().x, second));
        h.settle();
        assert!(
            (h.pill().y0 - (row_top + PALETTE_ROW_HEIGHT)).abs() < 0.001,
            "the pill followed the pointer"
        );
    }

    // ---- The controller ---------------------------------------------------

    #[test]
    fn the_controller_opens_closes_and_toggles_the_panel() {
        let controller = CommandPaletteController::new();
        assert!(!controller.is_open());
        assert!(controller.toggle(), "toggling a closed palette opens it");
        assert!(controller.is_open());
        assert!(!controller.toggle());
        controller.open();
        assert!(controller.is_open());
        controller.close();
        assert!(!controller.is_open());

        // A clone shares the same flag — an app's binding and its view hold two
        // handles onto one cell.
        let other = controller.clone();
        other.open();
        assert!(controller.is_open());
    }

    #[test]
    fn a_closed_palette_paints_nothing_and_an_opened_one_paints_its_panel() {
        let mut h = Harness::new();
        let closed = h.read_after(0.0);
        assert!(
            !closed
                .rrects
                .iter()
                .any(|(_, _, r, _)| *r == PALETTE_RADIUS),
            "a closed palette paints no panel"
        );
        h.open();
        let open = h.read();
        assert!(open.rrects.iter().any(|(_, _, r, _)| *r == PALETTE_RADIUS));
        assert!(!open.shadows.is_empty(), "the panel casts the glass shadow");
        assert!(open.strokes > 0, "the hairline and the search mark");
        assert!(
            open.inks.len() >= ROWS_UNFILTERED,
            "every row shaped its own run: {}",
            open.inks.len()
        );
    }

    // ---- Motion -----------------------------------------------------------

    #[test]
    fn the_rows_arrive_staggered_and_reduce_motion_lands_them_together() {
        // Mid-entrance, a staggered run has each row at its own alpha and the
        // tail has not started at all; the collapsed run has every row on one
        // beat.
        let mut h = Harness::new();
        h.controller.open();
        h.step(0.0);
        let staggered = h.read_after(PALETTE_STAGGER.as_secs_f64() * 1_000.0 * 2.5);
        assert!(
            staggered.layers.len() < ROWS_UNFILTERED,
            "the tail of the list has not started yet: {:?}",
            staggered.layers
        );
        let leader = staggered.layers[0];
        assert!(
            staggered.layers.iter().any(|alpha| *alpha != leader),
            "each row is at its own point in the run: {:?}",
            staggered.layers
        );

        let mut reduced = Harness::themed(reduced());
        reduced.controller.open();
        reduced.step(0.0);
        let flat = reduced.read_after(PALETTE_STAGGER.as_secs_f64() * 1_000.0 * 2.5);
        assert_eq!(
            flat.layers.len(),
            ROWS_UNFILTERED,
            "every row is moving at once: {:?}",
            flat.layers
        );
        assert!(
            flat.layers.iter().all(|alpha| *alpha == flat.layers[0]),
            "...and all at the same alpha: {:?}",
            flat.layers
        );
    }
}
