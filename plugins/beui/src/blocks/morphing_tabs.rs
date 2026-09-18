//! Ports beUI's `morphing-tabs` composed block —
//! `components/motion/morphing-tabs.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `morphing-tabs`: *"Reorderable tabs whose selected item grows into a
//! white content surface, with the active shape gliding as tabs move and a
//! shared morph between rooms."*
//!
//! | upstream | here |
//! |---|---|
//! | `MAX_TAB_WIDTH = 176`, `MIN_TAB_WIDTH = 96` | [`MORPHING_TABS_MAX_TAB_WIDTH`], [`MORPHING_TABS_MIN_TAB_WIDTH`] |
//! | `TAB_HEIGHT = 56`, `TAB_TOP = 24`, `RAIL_HEIGHT = 80` | [`MORPHING_TABS_TAB_HEIGHT`], [`MORPHING_TABS_TAB_TOP`], [`MORPHING_TABS_RAIL_HEIGHT`] |
//! | `TAB_RADIUS = 24`, `PANEL_RADIUS = 28`, `LIQUID_JOIN = 24` | [`MORPHING_TABS_TAB_RADIUS`], [`MORPHING_TABS_PANEL_RADIUS`], [`MORPHING_TABS_LIQUID_JOIN`] |
//! | `SURFACE_INSET = 16` (`mx-4`), `DRAG_THRESHOLD = 5` | [`MORPHING_TABS_SURFACE_INSET`], [`MORPHING_TABS_DRAG_THRESHOLD`] |
//! | rail `gap-3 md:gap-4`, root `rounded-[2rem]` | [`MORPHING_TABS_TAB_GAP`], [`MORPHING_TABS_ROOT_RADIUS`] |
//! | tab `px-3 gap-2`, icon `size-8`, label `text-base` | [`MORPHING_TABS_TAB_PADDING_X`], [`MORPHING_TABS_ICON_GAP`], [`MORPHING_TABS_ICON_BOX`], [`MORPHING_TABS_LABEL_SIZE`] |
//! | inactive tab `inset-x-0 bottom-2 top-0 rounded-[1.25rem]` | [`MORPHING_TABS_INACTIVE_RADIUS`], [`MORPHING_TABS_INACTIVE_BOTTOM`] |
//! | `hover:bg-white/[0.06]`, dragging `bg-[#3a3a3a]` | [`MORPHING_TABS_HOVER_WASH`], [`MORPHING_TABS_DRAG_WASH`] |
//! | panel `mx-4 min-h-64 rounded-[1.75rem]` | [`MORPHING_TABS_PANEL_MIN_HEIGHT`], [`MORPHING_TABS_PANEL_RADIUS`] |
//! | close button `size-6 right-2` | [`MORPHING_TABS_CLOSE_BOX`], [`MORPHING_TABS_CLOSE_INSET`] |
//! | tab travel, drag settle, surface glide: `SPRING_GLIDE` | [`MORPHING_TABS_GLIDE`] |
//! | panel `enter {y: 8, blur 6}` on `SPRING_PRESS` | [`MORPHING_TABS_CONTENT_ENTER`], [`MORPHING_TABS_CONTENT_RISE`] |
//! | panel `exit {y: -5, blur 5, 0.12s EASE_OUT}` | [`MORPHING_TABS_CONTENT_EXIT`], [`MORPHING_TABS_CONTENT_EXIT_RISE`] |
//!
//! # Premise correction: the morph is an analytic path, not a gooey filter
//!
//! The porting card describes this slug as "the beUI signature gooey/blob
//! morph", and instructs the port to *"approximate the blob with animated
//! rounded-rect metrics … if upstream's SVG-filter gooeyness is unreproducible,
//! record the degradation"*. There is no SVG filter, no `feGaussianBlur`, no
//! `feColorMatrix` and no gooeyness anywhere in `morphing-tabs.tsx`.
//!
//! What it actually ships is `liquidTabPath` — a **single analytic `<path>`**
//! spanning the whole panel width, whose top edge dips up to enclose the active
//! tab and flares back down to the panel line through two cubic Béziers
//! ([`MORPHING_TABS_LIQUID_JOIN`] wide, control points at `0.55` of the depth).
//! That is a closed-form shape made of lines, quadratics and cubics, so it ports
//! **exactly** to a `kurbo::BezPath` — [`morphing_tabs_liquid_path`] is a
//! statement-for-statement transcription of it, filled through
//! `PaintScene::fill_path`. No approximation, no rounded-rect stand-in, and no
//! degradation to record: this port is more faithful than the card asked for,
//! not less.
//!
//! The path is what carries the whole signature effect. As the active tab
//! glides, `liquidTabPath` is re-evaluated at the travelling `left`, so the
//! notch slides along the panel edge and the two flares stretch and compress
//! with it — the "liquid" read. [`morphing_tabs_liquid_path`] is public so a
//! consumer or a test can evaluate the same geometry the paint pass does.
//!
//! # Premise correction: reordering is half the component
//!
//! The card's spec note covers only the indicator morph. The registry entry
//! ("Reorderable tabs …", keywords `drag tabs react`) and the source both put
//! **pointer drag-to-reorder** at the centre: a 940-line file of which the
//! reorder session, the slot-midpoint swap rule, the displaced-neighbour
//! bookkeeping and the `Alt`+arrow keyboard reorder are the larger half. All of
//! it is ported.
//!
//! # The colours are re-tokenised
//!
//! Upstream hardcodes four hexes — `#292929` (shell), `#fafaf8` (surface and
//! panel), `#181818` (panel ink) and `#3a3a3a` (a dragging tab) — none of which
//! is a beUI token. The catalog resolves colour from tokens only, so the shell
//! and panel take the *inverted* chrome pair the
//! [dynamic island](crate::blocks::dynamic_island) already establishes for the
//! same shape (ink surface, background-coloured content), and the two flat
//! greys become washes over the shell: `#3a3a3a` over `#292929` is about a 7%
//! white lift, recorded as [`MORPHING_TABS_DRAG_WASH`] beside upstream's own
//! literal `white/[0.06]` hover.
//!
//! # A layout animation, timed at paint and applied at layout
//!
//! Every travelling quantity here — each tab's slot position, the dragged tab's
//! own travel, the liquid surface's `left` — is *geometry inside a fixed box*,
//! and the box itself never resizes with the motion. So unlike this file's
//! siblings, `paint` asks for a bare `request_frame`, not a relayout: the rail
//! is 80px tall whatever the tabs are doing, and only the panel's measured
//! height (which the crossfade does not change) reaches layout.
//!
//! # Controlled value, uncontrolled order
//!
//! That split is upstream's. `value` is a prop reported through
//! `on_value_change`; `order` is component state that starts in declaration
//! order, is reconciled against the item list on every rebuild (retained ids
//! keep their places, new ones append), and is *reported* through
//! `on_order_change` after a drag or an `Alt`+arrow. A caller that wants to own
//! the order re-declares its items in the reported order and the reconciliation
//! is a no-op.
//!
//! # Degradations against the web original
//!
//! - **No blur.** Both panel stages animate `filter: blur(6px)/blur(5px) → 0`;
//!   `PaintScene` publishes no blur filter, so the fade and the rise carry it.
//! - **The order commits on release, not after the neighbours settle.**
//!   Upstream awaits a `requestAnimationFrame` loop (capped at 500ms) that waits
//!   for every displaced tab's spring to come to rest before it swaps the array,
//!   purely to avoid a one-frame flicker. Here the commit happens on release.
//!   Nothing moves at that moment — the displaced tabs were already animated to
//!   their new slots *during* the drag, so the committed order matches what is
//!   on screen — which is what makes the wait unnecessary rather than merely
//!   skipped.
//! - **The keyboard moves a widget-local roving index, not focus.** Upstream
//!   `focus()`es the next tab button; the whole block is one widget here, so the
//!   arrows move activation and a local highlight — the same call
//!   [`crate::blocks::expandable_action_bar`] makes.
//! - **No `aria-controls`/`id` wiring.** Those pair a `tabpanel` with its `tab`
//!   by DOM id; the semantics tree pairs them by containment instead.
//! - **No fallback effect.** Upstream runs an effect that *writes* `value` when
//!   the current one names no item. A widget here does not report a value nobody
//!   asked for; it simply falls back to the first enabled tab for display, so an
//!   unmatched `value` shows that tab without a spurious callback.
//! - **The rail does not scroll, by design.** That is upstream's own stated
//!   constraint (the liquid surface is one continuous shape, so its notch must
//!   stay over the tab that cut it) and the three-tier width fit is its answer.
//!   Ported whole, including the tier where the floor itself gives way.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod,
    Color, ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx,
    NamedKey, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View,
    Widget, any, build_child, erase_callback_arg, rebuild_child, teardown_child, text::TextStyle,
};
use frust::{FrameTime, Theme};

use crate::motion::{Presence, PresencePhase, Ramp};
use crate::press::{Lane, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::{EASE_OUT, SPRING_GLIDE, SPRING_PRESS};

// ---- Metrics ---------------------------------------------------------------

/// A tab's design width, in logical px (`MAX_TAB_WIDTH = 176`).
pub const MORPHING_TABS_MAX_TAB_WIDTH: f64 = 176.0;

/// The narrowest a tab may shrink to before its label stops reading, in logical
/// px (`MIN_TAB_WIDTH = 96`).
pub const MORPHING_TABS_MIN_TAB_WIDTH: f64 = 96.0;

/// A tab's height, in logical px (`TAB_HEIGHT = 56`).
pub const MORPHING_TABS_TAB_HEIGHT: f64 = 56.0;

/// The rail room above a tab, in logical px (`TAB_TOP = 24`).
pub const MORPHING_TABS_TAB_TOP: f64 = 24.0;

/// The rail's height, in logical px (`RAIL_HEIGHT = 80`).
pub const MORPHING_TABS_RAIL_HEIGHT: f64 = 80.0;

/// A tab's top corner radius, in logical px (`TAB_RADIUS = 24`).
pub const MORPHING_TABS_TAB_RADIUS: f64 = 24.0;

/// The panel's corner radius, in logical px (`PANEL_RADIUS = 28`).
pub const MORPHING_TABS_PANEL_RADIUS: f64 = 28.0;

/// How wide each flare beside the active tab is, in logical px
/// (`LIQUID_JOIN = 24`) — the whole "liquid" read.
pub const MORPHING_TABS_LIQUID_JOIN: f64 = 24.0;

/// The panel's inset from the shell edge, in logical px (`SURFACE_INSET = 16`,
/// `mx-4`).
pub const MORPHING_TABS_SURFACE_INSET: f64 = 16.0;

/// The shell's corner radius, in logical px (`rounded-[2rem]`).
pub const MORPHING_TABS_ROOT_RADIUS: f64 = 32.0;

/// The gap between two tab slots, in logical px (`gap-3`; upstream reads the
/// live `columnGap`, which is `gap-4` from the `md` breakpoint up).
pub const MORPHING_TABS_TAB_GAP: f64 = 12.0;

/// How far a pointer must travel before a press becomes a reorder drag, in
/// logical px (`DRAG_THRESHOLD = 5`).
pub const MORPHING_TABS_DRAG_THRESHOLD: f64 = 5.0;

/// The panel's minimum height, in logical px (`min-h-64`).
pub const MORPHING_TABS_PANEL_MIN_HEIGHT: f64 = 256.0;

/// An inactive tab's own radius, in logical px (`rounded-[1.25rem]`).
pub const MORPHING_TABS_INACTIVE_RADIUS: f64 = 20.0;

/// How far short of the rail line an inactive tab's fill stops, in logical px
/// (`bottom-2`).
pub const MORPHING_TABS_INACTIVE_BOTTOM: f64 = 8.0;

/// A tab's horizontal padding, in logical px (`px-3`).
pub const MORPHING_TABS_TAB_PADDING_X: f64 = 12.0;

/// A tab icon's box, in logical px (`size-8`).
pub const MORPHING_TABS_ICON_BOX: f64 = 32.0;

/// The gap between a tab's icon and its label, in logical px (`gap-2`).
pub const MORPHING_TABS_ICON_GAP: f64 = 8.0;

/// A tab label's type size, in logical px (`text-base`).
pub const MORPHING_TABS_LABEL_SIZE: f64 = 16.0;

/// The close affordance's box, in logical px (`size-6`).
pub const MORPHING_TABS_CLOSE_BOX: f64 = 24.0;

/// Its inset from the tab's trailing edge, in logical px (`right-2`).
pub const MORPHING_TABS_CLOSE_INSET: f64 = 8.0;

/// An inactive tab's hover wash (`hover:bg-white/[0.06]`).
pub const MORPHING_TABS_HOVER_WASH: f32 = 0.06;

/// A dragging tab's wash.
///
/// Upstream paints the flat `#3a3a3a` over its flat `#292929` shell — about a
/// 7% white lift, rounded here to a tenth so the wash is a stated fraction
/// rather than a re-derived hex (see the [module docs](self)).
pub const MORPHING_TABS_DRAG_WASH: f32 = 0.10;

/// An inactive tab label's alpha (`text-white/70`).
pub const MORPHING_TABS_INACTIVE_INK_ALPHA: f32 = 0.70;

// ---- Motion ----------------------------------------------------------------

/// The one spring the whole rail moves on: every tab's slot travel, the dragged
/// tab's release settle and the liquid surface's glide (`SPRING_GLIDE`).
pub const MORPHING_TABS_GLIDE: Ramp = Ramp::spring(SPRING_GLIDE);

/// The panel entrance (`SPRING_PRESS`).
pub const MORPHING_TABS_CONTENT_ENTER: Ramp = Ramp::spring(SPRING_PRESS);

/// The panel exit: `{ duration: 0.12, ease: EASE_OUT }`.
pub const MORPHING_TABS_CONTENT_EXIT: Ramp = Ramp::eased(Duration::from_millis(120), EASE_OUT);

/// How far below its resting place an entering panel starts, in logical px
/// (`initial: { y: 8 }`).
pub const MORPHING_TABS_CONTENT_RISE: f64 = 8.0;

/// How far above its resting place a leaving panel ends, in logical px
/// (`exit: { y: -5 }`).
pub const MORPHING_TABS_CONTENT_EXIT_RISE: f64 = 5.0;

// ---- The liquid surface ----------------------------------------------------

/// The signature shape: the panel's top edge with a notch cut for the active
/// tab, flared into it by two cubics.
///
/// A statement-for-statement transcription of upstream's `liquidTabPath`
/// (`tabLeft`, `surfaceWidth`, `tabWidth` are its three arguments, unchanged).
/// See the [module docs](self) for why this is an exact port rather than the
/// rounded-rect approximation the porting card expected.
///
/// The returned path is in **rail-local** coordinates: `y = 0` is the shell's
/// top edge, `y = `[`MORPHING_TABS_RAIL_HEIGHT`] is the rail line the panel
/// meets, and the shape runs [`MORPHING_TABS_PANEL_RADIUS`] past it so the
/// panel's own rounded corners are covered from behind.
pub fn morphing_tabs_liquid_path(tab_left: f64, surface_width: f64, tab_width: f64) -> BezPath {
    let panel_left = MORPHING_TABS_SURFACE_INSET;
    let panel_right = surface_width - MORPHING_TABS_SURFACE_INSET;
    let left = tab_left.min(panel_right - tab_width).max(panel_left);
    let right = left + tab_width;
    let top = MORPHING_TABS_RAIL_HEIGHT - MORPHING_TABS_TAB_HEIGHT;
    let bottom = MORPHING_TABS_RAIL_HEIGHT;
    let left_join = (left - MORPHING_TABS_LIQUID_JOIN).max(panel_left);
    let right_join = (right + MORPHING_TABS_LIQUID_JOIN).min(panel_right);
    let left_depth = MORPHING_TABS_LIQUID_JOIN.min(left - left_join);
    let right_depth = MORPHING_TABS_LIQUID_JOIN.min(right_join - right);
    let left_control = left_depth * 0.55;
    let right_control = right_depth * 0.55;
    let left_panel_radius = MORPHING_TABS_PANEL_RADIUS.min(left_join - panel_left);
    let right_panel_radius = MORPHING_TABS_PANEL_RADIUS.min(panel_right - right_join);

    let mut path = BezPath::new();
    path.move_to((panel_left, bottom + MORPHING_TABS_PANEL_RADIUS));
    path.line_to((panel_left, bottom + left_panel_radius));
    path.quad_to(
        (panel_left, bottom),
        (panel_left + left_panel_radius, bottom),
    );
    path.line_to((left_join, bottom));
    path.curve_to(
        (left_join + left_control, bottom),
        (left, bottom - left_depth + left_control),
        (left, bottom - left_depth),
    );
    path.line_to((left, top + MORPHING_TABS_TAB_RADIUS));
    path.quad_to((left, top), (left + MORPHING_TABS_TAB_RADIUS, top));
    path.line_to((right - MORPHING_TABS_TAB_RADIUS, top));
    path.quad_to((right, top), (right, top + MORPHING_TABS_TAB_RADIUS));
    path.line_to((right, bottom - right_depth));
    path.curve_to(
        (right, bottom - right_depth + right_control),
        (right_join - right_control, bottom),
        (right_join, bottom),
    );
    path.line_to((panel_right - right_panel_radius, bottom));
    path.quad_to(
        (panel_right, bottom),
        (panel_right, bottom + right_panel_radius),
    );
    path.line_to((panel_right, bottom + MORPHING_TABS_PANEL_RADIUS));
    path.close_path();
    path
}

/// The fitted tab width and slot gap for `count` tabs across `surface_width`.
///
/// Upstream's three tiers of sacrifice, in its own order: the design width first
/// (down to the floor where a truncated label still reads), then the gap
/// between slots, then the floor itself — *"a cramped tab is still tappable, a
/// clipped one is not reachable at all"*. Every tier fits inside the panel by
/// construction, which is what lets the liquid notch stay over the tab that cut
/// it without ever being clamped away.
pub fn morphing_tabs_fit(count: usize, surface_width: f64, gap: f64) -> (f64, f64) {
    if surface_width <= 0.0 || count == 0 {
        return (MORPHING_TABS_MAX_TAB_WIDTH, gap);
    }
    let inner = surface_width - MORPHING_TABS_SURFACE_INSET * 2.0;
    let width_at = |gap: f64| ((inner - gap * (count - 1) as f64) / count as f64).floor();

    if width_at(gap) >= MORPHING_TABS_MIN_TAB_WIDTH {
        return (MORPHING_TABS_MAX_TAB_WIDTH.min(width_at(gap)), gap);
    }
    if count > 1 && width_at(0.0) >= MORPHING_TABS_MIN_TAB_WIDTH {
        let fitted =
            ((inner - MORPHING_TABS_MIN_TAB_WIDTH * count as f64) / (count - 1) as f64).floor();
        return (MORPHING_TABS_MIN_TAB_WIDTH, fitted.max(0.0));
    }
    (width_at(0.0).max(0.0), 0.0)
}

// ---- The view --------------------------------------------------------------

/// One tab: its identity, its label, an optional icon view, and the room it
/// opens.
pub struct MorphingTabsItem<State: 'static> {
    id: String,
    label: String,
    icon: Option<AnyView<State>>,
    content: AnyView<State>,
    disabled: bool,
}

/// A tab identified by `id`, labelled `label`, opening `content`.
pub fn morphing_tabs_item<State: 'static, V: View<State>>(
    id: impl Into<String>,
    label: impl Into<String>,
    content: V,
) -> MorphingTabsItem<State> {
    MorphingTabsItem {
        id: id.into(),
        label: label.into(),
        icon: None,
        content: any(content),
        disabled: false,
    }
}

impl<State: 'static> MorphingTabsItem<State> {
    /// The view drawn before the label (`item.icon`).
    pub fn icon<V: View<State>>(mut self, icon: V) -> Self {
        self.icon = Some(any(icon));
        self
    }

    /// Make this tab inert: unactivatable, undraggable, skipped by nothing else
    /// (`item.disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// This tab's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Every child view this tab owns, in pod order: the icon, then the room.
    fn child_views(&self) -> Vec<&AnyView<State>> {
        match &self.icon {
            Some(icon) => vec![icon, &self.content],
            None => vec![&self.content],
        }
    }
}

/// A view-held selection callback, erased on build.
type OnValueChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held order callback, erased on build.
type OnOrderChange<State> = Rc<dyn Fn(&mut State, Vec<String>)>;

/// A view-held close callback, erased on build.
type OnClose<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI morphing tab set. See the [module docs](self).
pub struct MorphingTabsView<State: 'static> {
    value: Option<String>,
    items: Vec<MorphingTabsItem<State>>,
    on_value_change: OnValueChange<State>,
    on_order_change: Option<OnOrderChange<State>>,
    on_close: Option<OnClose<State>>,
}

/// Create a morphing tab set whose open tab is the one whose id equals `value`,
/// reporting a requested value through `on_value_change` — a **controlled**
/// value over an **uncontrolled** order (see the [module docs](self)).
pub fn morphing_tabs<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: Option<String>,
    items: Vec<MorphingTabsItem<State>>,
    on_value_change: F,
) -> MorphingTabsView<State> {
    MorphingTabsView {
        value,
        items,
        on_value_change: Rc::new(on_value_change),
        on_order_change: None,
        on_close: None,
    }
}

impl<State: 'static> MorphingTabsView<State> {
    /// Report the new order after a drag or an `Alt`+arrow (`onOrderChange`).
    pub fn on_order_change<F: Fn(&mut State, Vec<String>) + 'static>(mut self, f: F) -> Self {
        self.on_order_change = Some(Rc::new(f));
        self
    }

    /// Give every tab a close affordance and report presses on it (`onClose`).
    pub fn on_close<F: Fn(&mut State, String) + 'static>(mut self, f: F) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }

    /// Every child view the set owns, tab by tab.
    fn child_views(&self) -> Vec<&AnyView<State>> {
        self.items
            .iter()
            .flat_map(|item| item.child_views())
            .collect()
    }

    /// Whether two item lists have the same shape — same ids, same icon slots.
    fn same_shape(&self, other: &Self) -> bool {
        self.items.len() == other.items.len()
            && self
                .items
                .iter()
                .zip(other.items.iter())
                .all(|(a, b)| a.id == b.id && a.icon.is_some() == b.icon.is_some())
    }
}

/// The resolved palette.
struct MorphingTabsColors {
    /// The shell (`bg-[#292929]` → the inverted chrome role).
    shell: Color,
    /// The liquid surface and the panel (`#fafaf8`).
    surface: Color,
    /// Ink on that surface (`text-[#181818]`).
    on_surface: Color,
    /// Ink on the shell (`text-white`).
    on_shell: Color,
}

/// Resolve the palette, falling back to the vendored **light** table with no
/// theme threaded — the unthemed posture every component in this catalog takes.
fn resolve_colors(theme: Option<&Theme>) -> MorphingTabsColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            MorphingTabsColors {
                shell: s.on_surface,
                surface: s.surface,
                on_surface: s.on_surface,
                on_shell: s.surface,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            MorphingTabsColors {
                shell: p.foreground,
                surface: p.background,
                on_surface: p.foreground,
                on_shell: p.background,
            }
        }
    }
}

/// The tab-label style (`text-base font-medium`), in the theme's
/// `label_large` family.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    themed_style(
        crate::text::label_style(MORPHING_TABS_LABEL_SIZE),
        ThemeTextType::LabelLarge,
        theme,
    )
}

/// Move `from` to `to` in `order` (`moveItem`).
fn move_item(order: &[usize], from: usize, to: usize) -> Vec<usize> {
    let mut next = order.to_vec();
    if from == to || from >= next.len() || to >= next.len() {
        return next;
    }
    let item = next.remove(from);
    next.insert(to, item);
    next
}

/// One retained tab.
struct TabEntry {
    id: String,
    label: LabelRun,
    disabled: bool,
    /// The pod holding this tab's icon, if it has one.
    icon_pod: Option<usize>,
    /// The pod holding this tab's room.
    content_pod: usize,
    /// The tab's own left edge, gliding toward its slot on
    /// [`MORPHING_TABS_GLIDE`].
    position: Lane,
    /// The room's enter/exit driver.
    presence: Presence,
}

/// A live reorder drag (`DragSession`).
#[derive(Clone, Debug)]
struct DragSession {
    /// The dragged tab's entry index.
    entry: usize,
    /// Where the press started, widget-local.
    origin_x: f64,
    /// The slot the tab started in.
    start_index: usize,
    /// The slot it would land in right now.
    target_index: usize,
    /// Whether the pointer has passed [`MORPHING_TABS_DRAG_THRESHOLD`].
    moved: bool,
    /// The slot lefts captured when the drag began — upstream captures them so
    /// a re-fit mid-drag cannot move the ground under the gesture.
    slot_lefts: Vec<f64>,
    /// The dragged tab's live left edge.
    left: f64,
    /// Whether the press landed on the close affordance instead.
    closing: bool,
}

impl<State: 'static> View<State> for MorphingTabsView<State> {
    type Element = MorphingTabsWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MorphingTabsWidget {
        let mut pods = Vec::new();
        let mut entries = Vec::with_capacity(self.items.len());
        for item in &self.items {
            let icon_pod = item.icon.as_ref().map(|icon| {
                pods.push(build_child(icon, ctx));
                pods.len() - 1
            });
            pods.push(build_child(&item.content, ctx));
            entries.push(TabEntry {
                id: item.id.clone(),
                label: LabelRun::new(item.label.clone()),
                disabled: item.disabled,
                icon_pod,
                content_pod: pods.len() - 1,
                position: Lane::at_rest(MORPHING_TABS_GLIDE, MORPHING_TABS_SURFACE_INSET),
                presence: Presence::new(MORPHING_TABS_CONTENT_ENTER, MORPHING_TABS_CONTENT_EXIT),
            });
        }
        let order = (0..entries.len()).collect();
        let mut widget = MorphingTabsWidget {
            entries,
            pods,
            order,
            value: self.value.clone(),
            has_close: self.on_close.is_some(),
            surface_width: 0.0,
            tab_width: MORPHING_TABS_MAX_TAB_WIDTH,
            slot_gap: MORPHING_TABS_TAB_GAP,
            panel_height: MORPHING_TABS_PANEL_MIN_HEIGHT,
            surface: Lane::at_rest(MORPHING_TABS_GLIDE, MORPHING_TABS_SURFACE_INSET),
            drag: None,
            focused: 0,
            hovered: None,
            placed: false,
            on_value_change: erase_callback_arg(&self.on_value_change),
            on_order_change: self.on_order_change.as_ref().map(erase_callback_arg),
            on_close: self.on_close.as_ref().map(erase_callback_arg),
        };
        widget.focused = widget.active().unwrap_or(0);
        widget.sync_presences();
        widget.settle_initial();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MorphingTabsWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        element.on_order_change = self.on_order_change.as_ref().map(erase_callback_arg);
        element.on_close = self.on_close.as_ref().map(erase_callback_arg);
        element.has_close = self.on_close.is_some();
        let mut flags = ChangeFlags::NONE;

        if self.same_shape(prev) {
            for (pod, (previous, next)) in element
                .pods
                .iter_mut()
                .zip(prev.child_views().into_iter().zip(self.child_views()))
            {
                flags |= rebuild_child(next, previous, pod, ctx);
            }
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.label.set_content(item.label.clone()) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.disabled != item.disabled {
                    entry.disabled = item.disabled;
                    flags |= ChangeFlags::PAINT;
                }
            }
        } else {
            for (view, pod) in prev.child_views().into_iter().zip(element.pods.iter_mut()) {
                teardown_child(view, pod, ctx);
            }
            let mut pods = Vec::new();
            let mut entries = Vec::with_capacity(self.items.len());
            for item in &self.items {
                let icon_pod = item.icon.as_ref().map(|icon| {
                    pods.push(build_child(icon, ctx));
                    pods.len() - 1
                });
                pods.push(build_child(&item.content, ctx));
                entries.push(TabEntry {
                    id: item.id.clone(),
                    label: LabelRun::new(item.label.clone()),
                    disabled: item.disabled,
                    icon_pod,
                    content_pod: pods.len() - 1,
                    position: Lane::at_rest(MORPHING_TABS_GLIDE, MORPHING_TABS_SURFACE_INSET),
                    presence: Presence::new(
                        MORPHING_TABS_CONTENT_ENTER,
                        MORPHING_TABS_CONTENT_EXIT,
                    ),
                });
            }
            element.entries = entries;
            element.pods = pods;
            element.drag = None;
            element.hovered = None;
            element.placed = false;
            element.reconcile_order();
            element.sync_presences();
            element.settle_initial();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.value != self.value {
            element.value = self.value.clone();
            if let Some(index) = element.active() {
                element.focused = index;
            }
            element.sync_presences();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MorphingTabsWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.child_views().into_iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// The retained widget for a [`MorphingTabsView`].
pub struct MorphingTabsWidget {
    entries: Vec<TabEntry>,
    /// Every tab's child views, flattened: icon (when present) then room.
    pods: Vec<ChildPod>,
    /// The rail's own order, as entry indices — component state, reported
    /// through `on_order_change` rather than driven by a prop.
    order: Vec<usize>,
    /// The app-confirmed value.
    value: Option<String>,
    /// Whether the close affordance is shown at all (`onClose` provided).
    has_close: bool,
    /// The shell's width, from the last layout.
    surface_width: f64,
    /// The fitted tab width, from the last layout.
    tab_width: f64,
    /// The fitted slot gap, from the last layout.
    slot_gap: f64,
    /// The panel's measured height, from the last layout.
    panel_height: f64,
    /// The liquid surface's own left edge, gliding after the active tab.
    surface: Lane,
    /// The reorder drag in flight.
    drag: Option<DragSession>,
    /// The tab the roving highlight sits on.
    focused: usize,
    /// The latched hovered tab, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// Whether the slot lanes have ever been placed.
    placed: bool,
    on_value_change: ErasedArgCallback<String>,
    on_order_change: Option<ErasedArgCallback<Vec<String>>>,
    on_close: Option<ErasedArgCallback<String>>,
}

impl MorphingTabsWidget {
    /// The first tab that is not disabled (`firstEnabledItem`).
    fn first_enabled(&self) -> Option<usize> {
        self.order
            .iter()
            .copied()
            .find(|index| !self.entries[*index].disabled)
            .or_else(|| self.order.first().copied())
    }

    /// The open tab's entry index: the one matching `value`, or the first
    /// enabled one when nothing matches (`activeItem` — minus the effect that
    /// would write the fallback back out; see the module docs).
    fn active(&self) -> Option<usize> {
        match self.value.as_deref() {
            Some(value) => self
                .entries
                .iter()
                .position(|e| e.id == value)
                .or_else(|| self.first_enabled()),
            None => self.first_enabled(),
        }
    }

    /// Apply the confirmed value to every room's presence.
    fn sync_presences(&mut self) {
        let active = self.active();
        for (index, entry) in self.entries.iter_mut().enumerate() {
            entry.presence.set_open(active == Some(index));
        }
    }

    /// Drive the open room straight to `Present` — `<AnimatePresence
    /// initial={false}>`.
    fn settle_initial(&mut self) {
        let settled =
            FrameTime::from_nanos(MORPHING_TABS_CONTENT_ENTER.settle().as_nanos() as u64 + 1);
        for entry in &mut self.entries {
            entry.presence.advance(FrameTime::ZERO);
            entry.presence.advance(settled);
        }
    }

    /// Keep the order in step with the entry list: retained slots keep their
    /// places, new entries append (upstream's `setOrder` effect).
    fn reconcile_order(&mut self) {
        let count = self.entries.len();
        let mut next: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|index| *index < count)
            .collect();
        for index in 0..count {
            if !next.contains(&index) {
                next.push(index);
            }
        }
        self.order = next;
    }

    /// The slot left edge at `slot` (`slotLefts`).
    fn slot_left(&self, slot: usize) -> f64 {
        MORPHING_TABS_SURFACE_INSET + slot as f64 * (self.tab_width + self.slot_gap)
    }

    /// Where entry `index` sits in the order.
    fn order_index(&self, entry: usize) -> Option<usize> {
        self.order.iter().position(|e| *e == entry)
    }

    /// The slot an ordered `index` *displays* in while a drag is displacing its
    /// neighbours (`visualIndexFor`).
    fn visual_index(&self, index: usize) -> usize {
        let Some(drag) = self.drag.as_ref().filter(|d| d.moved) else {
            return index;
        };
        let start = drag.start_index;
        let target = drag.target_index;
        if index == start {
            return target;
        }
        if target > start && index > start && index <= target {
            return index - 1;
        }
        if target < start && index >= target && index < start {
            return index + 1;
        }
        index
    }

    /// Entry `index`'s current left edge: the live drag's, or the lane's.
    fn tab_left(&self, entry: usize) -> f64 {
        match self.drag.as_ref() {
            Some(drag) if drag.moved && drag.entry == entry => drag.left,
            _ => self.entries[entry].position.value(),
        }
    }

    /// The liquid surface's current left edge (`liquidDriver`).
    fn surface_left(&self) -> f64 {
        let Some(active) = self.active() else {
            return MORPHING_TABS_SURFACE_INSET;
        };
        match self.drag.as_ref().filter(|d| d.moved) {
            // The dragged tab *is* the active one: the notch rides the finger.
            Some(drag) if drag.entry == active => drag.left,
            // Another tab is dragging: the notch follows the active tab as it is
            // displaced.
            Some(_) => self.entries[active].position.value(),
            None => self.surface.value(),
        }
    }

    /// Entry `index`'s box in widget-local coordinates.
    fn tab_rect(&self, entry: usize) -> Rect {
        Rect::from_origin_size(
            Point::new(self.tab_left(entry), MORPHING_TABS_TAB_TOP),
            Size::new(self.tab_width.max(0.0), MORPHING_TABS_TAB_HEIGHT),
        )
    }

    /// The close affordance's box inside `tab`, when one is shown.
    fn close_rect(&self, tab: Rect) -> Option<Rect> {
        if !self.has_close || tab.width() <= MORPHING_TABS_CLOSE_BOX {
            return None;
        }
        Some(Rect::from_origin_size(
            Point::new(
                tab.x1 - MORPHING_TABS_CLOSE_INSET - MORPHING_TABS_CLOSE_BOX,
                tab.y0 + (tab.height() - MORPHING_TABS_CLOSE_BOX) / 2.0,
            ),
            Size::new(MORPHING_TABS_CLOSE_BOX, MORPHING_TABS_CLOSE_BOX),
        ))
    }

    /// The panel's box in widget-local coordinates.
    fn panel_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::new(MORPHING_TABS_SURFACE_INSET, MORPHING_TABS_RAIL_HEIGHT),
            Size::new(
                (self.surface_width - MORPHING_TABS_SURFACE_INSET * 2.0).max(0.0),
                self.panel_height,
            ),
        )
    }

    /// The tab under a widget-local `pos`, front to back (the dragged tab, then
    /// the active one, then the rest — upstream's `zIndex` ladder).
    fn hit_tab(&self, pos: Point) -> Option<usize> {
        let mut candidates: Vec<usize> = self.order.clone();
        candidates.sort_by_key(|entry| self.paint_depth(*entry));
        candidates
            .into_iter()
            .rev()
            .find(|entry| self.tab_rect(*entry).contains(pos))
    }

    /// Entry `index`'s paint depth: `2` dragged, `1` active, `0` otherwise
    /// (`zIndex: isDragging ? 30 : isActive ? 20 : 1`).
    fn paint_depth(&self, entry: usize) -> u8 {
        if self
            .drag
            .as_ref()
            .is_some_and(|d| d.moved && d.entry == entry)
        {
            2
        } else if self.active() == Some(entry) {
            1
        } else {
            0
        }
    }

    /// Report entry `index`'s value (never assigning it to `self.value`).
    fn request_value(&mut self, ctx: &mut EventCtx, entry: usize) {
        let Some(item) = self.entries.get(entry) else {
            return;
        };
        if item.disabled {
            return;
        }
        let value = item.id.clone();
        (self.on_value_change)(ctx, value);
    }

    /// Commit `next` as the order and report it when it actually changed
    /// (`commitOrder`).
    fn commit_order(&mut self, ctx: &mut EventCtx, next: Vec<usize>) {
        if self.order == next {
            return;
        }
        self.order = next;
        if let Some(report) = self.on_order_change.as_mut() {
            let ids = self
                .order
                .iter()
                .map(|entry| self.entries[*entry].id.clone())
                .collect::<Vec<_>>();
            report(ctx, ids);
        }
    }

    /// Move entry `entry` one slot in `direction` and report the new order
    /// (`moveBy`).
    fn move_by(&mut self, ctx: &mut EventCtx, entry: usize, direction: isize) {
        let Some(index) = self.order_index(entry) else {
            return;
        };
        let next_index = index as isize + direction;
        if next_index < 0 || next_index >= self.order.len() as isize {
            return;
        }
        if self.entries[entry].disabled {
            return;
        }
        let next = move_item(&self.order, index, next_index as usize);
        self.commit_order(ctx, next);
    }

    /// Re-aim every tab's slot lane at the place its *visual* index puts it —
    /// the displaced-neighbour bookkeeping.
    ///
    /// Called from `layout` and, crucially, from the drag itself: a drag asks
    /// only for a redraw (nothing it moves resizes the widget), so if this ran
    /// only at layout the neighbours would never step aside.
    fn retarget_slots(&mut self) {
        for index in 0..self.order.len() {
            let entry = self.order[index];
            let target = self.slot_left(self.visual_index(index));
            if self.placed {
                self.entries[entry].position.retarget(target);
            } else {
                self.entries[entry].position = Lane::at_rest(MORPHING_TABS_GLIDE, target);
            }
        }
    }

    /// Where a dragged tab would land, given its clamped visual left — exactly
    /// upstream's two midpoint loops.
    fn drag_target(&self, drag: &DragSession, visual_left: f64) -> usize {
        let start = drag.start_index;
        let lefts = &drag.slot_lefts;
        let mut target = start;
        if visual_left >= drag.slot_lefts[start] {
            for (index, left) in lefts.iter().enumerate().skip(start + 1) {
                if visual_left + self.tab_width / 2.0 >= *left {
                    target = index;
                }
            }
        } else {
            for index in (0..start).rev() {
                if visual_left <= lefts[index] + self.tab_width / 2.0 {
                    target = index;
                }
            }
        }
        target
    }

    /// Advance every lane and presence to `now`, returning whether anything is
    /// still moving.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.surface.snap();
            for entry in &mut self.entries {
                entry.position.snap();
                entry.presence = entry.presence.collapsed();
            }
        }
        let mut moving = self.surface.advance(now);
        for entry in &mut self.entries {
            moving |= entry.position.advance(now);
            entry.presence.advance(now);
            moving |= entry.presence.is_animating();
        }
        moving
    }

    /// The stage room `entry` is drawn at: `(alpha, rise)`.
    fn content_stage(&self, entry: usize, now: FrameTime) -> (f64, f64) {
        let presence = &self.entries[entry].presence;
        let p = presence.presence(now).clamp(0.0, 1.0);
        match presence.phase() {
            // The entrance rises *up* into place from below; the exit lifts out
            // above. Upstream authors the two signs separately.
            PresencePhase::Exiting => (p, -MORPHING_TABS_CONTENT_EXIT_RISE * (1.0 - p)),
            _ => (p, MORPHING_TABS_CONTENT_RISE * (1.0 - p)),
        }
    }
}

impl Widget for MorphingTabsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let text_style = label_style(theme);
        self.surface_width = crate::overlay::finite_or_zero(bc.max().width);
        let (tab_width, slot_gap) =
            morphing_tabs_fit(self.order.len(), self.surface_width, MORPHING_TABS_TAB_GAP);
        self.tab_width = tab_width;
        self.slot_gap = slot_gap;

        for entry in &mut self.entries {
            entry.label.layout(ctx, &text_style);
        }

        // Every room is measured, so the panel's height does not jump on a
        // switch; only a visible one is painted, routed to or published.
        let panel = self.panel_rect();
        let panel_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(
                panel.width(),
                (bc.max().height - MORPHING_TABS_RAIL_HEIGHT).max(0.0),
            ),
        );
        let mut content_height: f64 = 0.0;
        for entry in 0..self.entries.len() {
            let pod = self.entries[entry].content_pod;
            let size = self.pods[pod].layout_child(ctx, &panel_bc);
            content_height = content_height.max(size.height);
            self.pods[pod].set_origin(Point::new(panel.x0, panel.y0));
        }
        self.panel_height = MORPHING_TABS_PANEL_MIN_HEIGHT.max(content_height);

        // Retarget each tab's slot, and the surface's, from the visual order.
        self.retarget_slots();
        if let Some(active) = self.active()
            && let Some(index) = self.order_index(active)
        {
            let target = self.slot_left(self.visual_index(index));
            if self.placed && self.drag.is_none() {
                self.surface.retarget(target);
            } else if !self.placed {
                self.surface = Lane::at_rest(MORPHING_TABS_GLIDE, target);
            }
        }
        self.placed = true;

        let icon_bc =
            BoxConstraints::tight(Size::new(MORPHING_TABS_ICON_BOX, MORPHING_TABS_ICON_BOX));
        for index in 0..self.order.len() {
            let entry = self.order[index];
            if let Some(pod) = self.entries[entry].icon_pod {
                self.pods[pod].layout_child(ctx, &icon_bc);
                let rect = self.tab_rect(entry);
                self.pods[pod].set_origin(Point::new(
                    rect.x0 + MORPHING_TABS_TAB_PADDING_X,
                    rect.y0 + (rect.height() - MORPHING_TABS_ICON_BOX) / 2.0,
                ));
            }
        }

        bc.constrain(Size::new(
            self.surface_width,
            MORPHING_TABS_RAIL_HEIGHT + self.panel_height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let moving = self.advance(now, reduce_motion);
        let origin = ctx.origin();
        let size = ctx.size();
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }

        // The shell.
        let radius = style::resolve_radius(MORPHING_TABS_ROOT_RADIUS, size.width, size.height);
        scene.push_clip_rounded(origin, size, radius);
        scene.fill_rounded_rect(origin, size, radius, colors.shell);

        // The liquid surface — one continuous shape from the panel edge, up
        // around the active tab, and back down (see the module docs).
        if self.active().is_some()
            && self.surface_width > MORPHING_TABS_SURFACE_INSET * 2.0
            && self.tab_width > 0.0
        {
            let path =
                morphing_tabs_liquid_path(self.surface_left(), self.surface_width, self.tab_width);
            scene.fill_path(origin, &path, &Brush::Solid(colors.surface));
        }

        // The panel, over the surface's lower band — the two meet at the flares.
        let panel = self.panel_rect();
        if panel.width() > 0.0 && panel.height() > 0.0 {
            scene.fill_rounded_rect(
                origin + panel.origin().to_vec2(),
                panel.size(),
                style::resolve_radius(MORPHING_TABS_PANEL_RADIUS, panel.width(), panel.height()),
                colors.surface,
            );
        }
        self.paint_rooms(ctx, scene, now);

        // The tabs, back to front on upstream's own `zIndex` ladder.
        let mut painted: Vec<usize> = self.order.clone();
        painted.sort_by_key(|entry| self.paint_depth(*entry));
        for entry in painted {
            self.paint_tab(ctx, scene, &colors, entry);
        }
        scene.pop_clip();

        // Everything that travels here is geometry inside a box the motion
        // never resizes, so a bare frame request is enough.
        if moving {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.pods {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => self.handle_key(ctx, key),
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let active = self.active();
        let has_close = self.has_close;
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                // Published in the rail's own order, which is what a screen
                // reader should meet after a reorder.
                for entry in self.order.iter().copied() {
                    let item = &self.entries[entry];
                    let label = item.label.content().to_string();
                    let disabled = item.disabled;
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(label.as_str());
                        node.set_selected(active == Some(entry));
                        if disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                    if has_close {
                        let name = format!("Close {label}");
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(name.as_str());
                            node.add_action(Action::Click);
                        });
                    }
                }
            },
        );
        if let Some(pod) = active.map(|entry| self.entries[entry].content_pod) {
            self.pods[pod].semantics_child(ctx);
        }
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for pod in &self.pods {
            visitor(pod);
        }
    }
}

impl MorphingTabsWidget {
    /// Paint the open room (and any room still playing its exit), clipped to
    /// the panel.
    fn paint_rooms(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene, now: FrameTime) {
        let panel = self.panel_rect();
        if panel.width() <= 0.0 || panel.height() <= 0.0 {
            return;
        }
        let origin = ctx.origin();
        let at = origin + panel.origin().to_vec2();
        scene.push_clip_rounded(
            at,
            panel.size(),
            style::resolve_radius(MORPHING_TABS_PANEL_RADIUS, panel.width(), panel.height()),
        );
        for entry in 0..self.entries.len() {
            if !self.entries[entry].presence.is_visible() {
                continue;
            }
            let (alpha, rise) = self.content_stage(entry, now);
            let pod = self.entries[entry].content_pod;
            scene.push_layer(at, panel.size(), alpha as f32);
            scene.push_transform(Affine::translate((0.0, rise)));
            self.pods[pod].paint_child(ctx, scene);
            scene.pop_transform();
            scene.pop_layer();
        }
        scene.pop_clip();
    }

    /// Paint one tab: its own fill when inactive, its icon, its label and its
    /// close affordance. The active tab has no fill — the liquid surface is it.
    fn paint_tab(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        colors: &MorphingTabsColors,
        entry: usize,
    ) {
        let rect = self.tab_rect(entry);
        if rect.width() <= 0.0 {
            return;
        }
        let origin = ctx.origin();
        let at = origin + rect.origin().to_vec2();
        let is_active = self.active() == Some(entry);
        let is_dragging = self
            .drag
            .as_ref()
            .is_some_and(|d| d.moved && d.entry == entry);

        if !is_active {
            let wash = if is_dragging {
                MORPHING_TABS_DRAG_WASH
            } else if self.hovered == Some(entry) {
                MORPHING_TABS_HOVER_WASH
            } else {
                0.0
            };
            if wash > 0.0 {
                let fill = Size::new(
                    rect.width(),
                    (rect.height() - MORPHING_TABS_INACTIVE_BOTTOM).max(0.0),
                );
                scene.fill_rounded_rect(
                    at,
                    fill,
                    style::resolve_radius(MORPHING_TABS_INACTIVE_RADIUS, fill.width, fill.height),
                    style::with_alpha(colors.on_shell, wash),
                );
            }
        }

        if let Some(pod) = self.entries[entry].icon_pod {
            self.pods[pod].paint_child(ctx, scene);
        }

        let icon_width = self.entries[entry]
            .icon_pod
            .map_or(0.0, |_| MORPHING_TABS_ICON_BOX + MORPHING_TABS_ICON_GAP);
        let text_x = at.x + MORPHING_TABS_TAB_PADDING_X + icon_width;
        let close_room = self
            .close_rect(rect)
            .map_or(MORPHING_TABS_TAB_PADDING_X, |_| {
                MORPHING_TABS_CLOSE_INSET + MORPHING_TABS_CLOSE_BOX + MORPHING_TABS_ICON_GAP
            });
        let text_right = at.x + rect.width() - close_room;
        // An inactive tab's label sits `pb-2` higher, over the fill that stops
        // short of the rail line.
        let bottom_bias = if is_active {
            0.0
        } else {
            MORPHING_TABS_INACTIVE_BOTTOM
        };
        let label = self.entries[entry].label.size();
        let ink = if is_active {
            colors.on_surface
        } else if self.hovered == Some(entry) {
            colors.on_shell
        } else {
            style::with_alpha(colors.on_shell, MORPHING_TABS_INACTIVE_INK_ALPHA)
        };
        let dimmed = self.entries[entry].disabled;
        scene.push_clip(
            Point::new(text_x, at.y),
            Size::new((text_right - text_x).max(0.0), rect.height()),
        );
        self.entries[entry].label.paint(
            Point::new(
                text_x,
                at.y + (rect.height() - bottom_bias - label.height) / 2.0,
            ),
            style::disabled_tint(ink, dimmed, style::DISABLED_OPACITY),
            scene,
        );
        scene.pop_clip();

        if let Some(close) = self.close_rect(rect) {
            let mark = origin + close.origin().to_vec2();
            draw_close_mark(
                scene,
                mark,
                MORPHING_TABS_CLOSE_BOX,
                style::with_alpha(ink, MORPHING_TABS_INACTIVE_INK_ALPHA),
            );
        }
    }

    /// The keyboard arm: arrows move activation (wrapping), `Alt`+arrows
    /// reorder, `Space`/`Enter` activates the highlight.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> EventResult {
        let Some(direction) = arrow_step(key) else {
            if is_activation_key(key) {
                let entry = self.focused;
                self.request_value(ctx, entry);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        };
        if self.order.is_empty() {
            return EventResult::Ignored;
        }
        if key.modifiers.alt {
            let entry = self.focused;
            self.move_by(ctx, entry, direction);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let Some(index) = self.order_index(self.focused) else {
            return EventResult::Ignored;
        };
        let next_index =
            (index as isize + direction).rem_euclid(self.order.len() as isize) as usize;
        let next = self.order[next_index];
        self.focused = next;
        ctx.request_redraw();
        self.request_value(ctx, next);
        EventResult::Handled
    }

    /// The pointer arm: press, reorder drag, release.
    fn handle_pointer(
        &mut self,
        ctx: &mut EventCtx,
        p: &frust::authoring::PointerEvent,
    ) -> EventResult {
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(entry) = self.hit_tab(p.position) else {
                    return EventResult::Ignored;
                };
                let rect = self.tab_rect(entry);
                let closing = self
                    .close_rect(rect)
                    .is_some_and(|close| close.contains(p.position));
                if self.entries[entry].disabled && !closing {
                    return EventResult::Ignored;
                }
                let Some(start_index) = self.order_index(entry) else {
                    return EventResult::Ignored;
                };
                self.drag = Some(DragSession {
                    entry,
                    origin_x: p.position.x,
                    start_index,
                    target_index: start_index,
                    moved: false,
                    slot_lefts: (0..self.order.len()).map(|s| self.slot_left(s)).collect(),
                    left: self.slot_left(start_index),
                    closing,
                });
                self.focused = entry;
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(mut drag) = self.drag.clone() else {
                    let over = self.hit_tab(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                };
                // A press on the close affordance is a button press, never a
                // drag: upstream stops propagation on it.
                if drag.closing {
                    return EventResult::Handled;
                }
                let delta = p.position.x - drag.origin_x;
                if !drag.moved && delta.abs() < MORPHING_TABS_DRAG_THRESHOLD {
                    return EventResult::Handled;
                }
                if !drag.moved {
                    drag.moved = true;
                    // The surface stops gliding and joins the finger.
                    self.surface = Lane::at_rest(MORPHING_TABS_GLIDE, self.surface_left());
                }
                let min_left = drag.slot_lefts[0];
                let max_left = drag.slot_lefts[drag.slot_lefts.len() - 1];
                let start_left = drag.slot_lefts[drag.start_index];
                drag.left = (start_left + delta).clamp(min_left, max_left);
                drag.target_index = self.drag_target(&drag, drag.left);
                self.drag = Some(drag);
                // The neighbours step aside now, not at the next layout.
                self.retarget_slots();
                ctx.set_cursor(style::ACTIVE_CURSOR);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(drag) = self.drag.take() else {
                    return EventResult::Ignored;
                };
                if drag.closing {
                    let rect = self.tab_rect(drag.entry);
                    let re_hit = self
                        .close_rect(rect)
                        .is_some_and(|close| close.contains(p.position));
                    if re_hit && let Some(on_close) = self.on_close.as_mut() {
                        let id = self.entries[drag.entry].id.clone();
                        on_close(ctx, id);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if !drag.moved {
                    // A press that never became a drag activates the tab.
                    self.request_value(ctx, drag.entry);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                // The dragged tab settles onto its landing slot from where the
                // finger left it; its displaced neighbours are already there.
                let landing = drag.slot_lefts[drag.target_index];
                self.entries[drag.entry].position = Lane::at_rest(MORPHING_TABS_GLIDE, drag.left);
                self.entries[drag.entry].position.retarget(landing);
                let next = move_item(&self.order, drag.start_index, drag.target_index);
                self.commit_order(ctx, next);
                if self.active() == Some(drag.entry) {
                    self.surface = Lane::at_rest(MORPHING_TABS_GLIDE, drag.left);
                    self.surface.retarget(landing);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                let Some(drag) = self.drag.take() else {
                    return EventResult::Ignored;
                };
                if drag.moved {
                    // A taken-away gesture returns the tab to where it started.
                    let home = drag.slot_lefts[drag.start_index];
                    self.entries[drag.entry].position =
                        Lane::at_rest(MORPHING_TABS_GLIDE, drag.left);
                    self.entries[drag.entry].position.retarget(home);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

/// Draw Lucide's `X` in a `box`-wide square at `origin`.
///
/// Drawn rather than shaped, the route [`crate::components::animated_badge`]
/// takes for the same reason — this catalog bundles no icon set.
fn draw_close_mark(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let inset = box_size * 0.3;
    let brush = Brush::Solid(color);
    let mut path = BezPath::new();
    path.move_to((inset, inset));
    path.line_to((box_size - inset, box_size - inset));
    path.move_to((box_size - inset, inset));
    path.line_to((inset, box_size - inset));
    scene.stroke_path(origin, &path, 1.5, &brush);
}

/// Which way an arrow key moves along the rail, or `None`.
fn arrow_step(key: &KeyEvent) -> Option<isize> {
    match &key.key {
        Key::Named(NamedKey::ArrowRight) => Some(1),
        Key::Named(NamedKey::ArrowLeft) => Some(-1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Modifiers, PointerButton, PointerEvent, SemanticsUpdate};
    use frust::text;
    use kurbo::Shape as _;
    use std::any::Any;

    /// Records what the widget paints.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        paths: Vec<(Point, BezPath, Color)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn fill_path(&mut self, o: Point, p: &BezPath, b: &Brush) {
            if let Brush::Solid(color) = b {
                self.paths.push((o, p.clone(), *color));
            }
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_clip(&mut self, _o: Point, _s: Size) {}
        fn push_clip_rounded(&mut self, _o: Point, _s: Size, _r: f64) {}
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _t: Affine) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct App {
        value: Option<String>,
        value_calls: u32,
        order: Option<Vec<String>>,
        closed: Vec<String>,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<MorphingTabsItem<App>> {
        vec![
            morphing_tabs_item("inbox", "Inbox", text("the inbox room")).icon(text("I")),
            morphing_tabs_item("drafts", "Drafts", text("the drafts room")),
            morphing_tabs_item("archive", "Archive", text("the archive room")),
        ]
    }

    fn view(value: Option<&str>) -> MorphingTabsView<App> {
        morphing_tabs::<App, _>(
            value.map(str::to_string),
            items(),
            |s: &mut App, v: String| {
                s.value = Some(v);
                s.value_calls += 1;
            },
        )
        .on_order_change(|s: &mut App, ids: Vec<String>| s.order = Some(ids))
        .on_close(|s: &mut App, id: String| s.closed.push(id))
    }

    fn build_from(v: &MorphingTabsView<App>) -> MorphingTabsWidget {
        let mut counter = 0u64;
        View::<App>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout_at(w: &mut MorphingTabsWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(width, 900.0)),
        )
    }

    fn layout(w: &mut MorphingTabsWidget) -> Size {
        layout_at(w, 640.0)
    }

    fn laid_out(value: Option<&str>) -> (MorphingTabsWidget, Size) {
        let mut w = build_from(&view(value));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(w: &mut MorphingTabsWidget, size: Size, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn settle(w: &mut MorphingTabsWidget, size: Size, from_ms: f64) {
        for step in 0..25 {
            paint_at(w, size, from_ms + step as f64 * 200.0);
        }
    }

    fn retarget(w: &mut MorphingTabsWidget, from: Option<&str>, to: Option<&str>) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<App>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn key_with(named: NamedKey, alt: bool) -> InputEvent {
        let modifiers = Modifiers {
            alt,
            ..Modifiers::default()
        };
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers,
            repeat: false,
        })
    }

    fn dispatch(w: &mut MorphingTabsWidget, size: Size, event: &InputEvent, state: &mut App) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The rail lays its tabs out on the fitted slot ladder, the panel sits
    /// below it at `mx-4`, and the whole block is rail + panel tall.
    #[test]
    fn the_rail_and_panel_take_their_upstream_geometry() {
        let (w, size) = laid_out(Some("inbox"));
        assert_eq!(w.slot_left(0), MORPHING_TABS_SURFACE_INSET);
        assert_eq!(w.slot_left(1), w.slot_left(0) + w.tab_width + w.slot_gap);
        let tab = w.tab_rect(0);
        assert_eq!(tab.y0, MORPHING_TABS_TAB_TOP);
        assert_eq!(
            tab.y1, MORPHING_TABS_RAIL_HEIGHT,
            "the tab meets the rail line"
        );

        let panel = w.panel_rect();
        assert_eq!(panel.x0, MORPHING_TABS_SURFACE_INSET);
        assert_eq!(panel.x1, 640.0 - MORPHING_TABS_SURFACE_INSET);
        assert_eq!(panel.y0, MORPHING_TABS_RAIL_HEIGHT);
        assert_eq!(panel.height(), MORPHING_TABS_PANEL_MIN_HEIGHT);
        assert_eq!(
            size.height,
            MORPHING_TABS_RAIL_HEIGHT + MORPHING_TABS_PANEL_MIN_HEIGHT
        );
    }

    /// The three-tier width fit, each tier exercised at the width that selects
    /// it: the design width, then the gap, then the floor itself.
    #[test]
    fn the_width_fit_sacrifices_in_upstream_order() {
        // Tier 1 — roomy: the design width caps it and the gap survives whole.
        let (wide, gap) = morphing_tabs_fit(3, 1200.0, MORPHING_TABS_TAB_GAP);
        assert_eq!(wide, MORPHING_TABS_MAX_TAB_WIDTH);
        assert_eq!(gap, MORPHING_TABS_TAB_GAP);

        // Tier 1 — snug: under the design width but still over the floor.
        let (snug, gap) = morphing_tabs_fit(3, 400.0, MORPHING_TABS_TAB_GAP);
        assert!(snug > MORPHING_TABS_MIN_TAB_WIDTH && snug < MORPHING_TABS_MAX_TAB_WIDTH);
        assert_eq!(gap, MORPHING_TABS_TAB_GAP);

        // Tier 2 — the gap gives way so the floor survives.
        let (floored, gap) = morphing_tabs_fit(3, 320.0, MORPHING_TABS_TAB_GAP);
        assert_eq!(floored, MORPHING_TABS_MIN_TAB_WIDTH);
        assert!(gap < MORPHING_TABS_TAB_GAP);

        // Tier 3 — the floor itself gives way; a cramped tab beats a clipped one.
        let (cramped, gap) = morphing_tabs_fit(3, 240.0, MORPHING_TABS_TAB_GAP);
        assert!(cramped < MORPHING_TABS_MIN_TAB_WIDTH && cramped > 0.0);
        assert_eq!(gap, 0.0);

        // Every tier fits inside the panel by construction — the property the
        // liquid notch depends on.
        for width in [1200.0, 400.0, 320.0, 240.0] {
            let (tab, gap) = morphing_tabs_fit(3, width, MORPHING_TABS_TAB_GAP);
            let span = 3.0 * tab + 2.0 * gap;
            assert!(
                span <= width - MORPHING_TABS_SURFACE_INSET * 2.0 + 1e-9,
                "{width}: {span} does not fit"
            );
        }
    }

    /// The liquid path is the analytic shape upstream authors, not an
    /// approximation: it closes, it spans the panel, its notch encloses the
    /// active tab exactly, and it runs past the rail line to meet the panel.
    #[test]
    fn the_liquid_path_encloses_the_tab_and_meets_the_panel() {
        let width = 640.0;
        let tab_width = 176.0;
        let left = 200.0;
        let path = morphing_tabs_liquid_path(left, width, tab_width);
        let bbox = path.bounding_box();
        assert_eq!(
            bbox.x0, MORPHING_TABS_SURFACE_INSET,
            "spans from the panel edge"
        );
        assert_eq!(bbox.x1, width - MORPHING_TABS_SURFACE_INSET);
        assert_eq!(
            bbox.y0,
            MORPHING_TABS_RAIL_HEIGHT - MORPHING_TABS_TAB_HEIGHT,
            "reaches the tab's own top edge"
        );
        assert_eq!(
            bbox.y1,
            MORPHING_TABS_RAIL_HEIGHT + MORPHING_TABS_PANEL_RADIUS,
            "and past the rail line, behind the panel's corners"
        );

        // The notch really is over the tab: a point just inside the tab box is
        // filled, one beside it above the rail line is not.
        let inside = Point::new(left + tab_width / 2.0, MORPHING_TABS_RAIL_HEIGHT - 8.0);
        let beside = Point::new(
            left - MORPHING_TABS_LIQUID_JOIN - 20.0,
            MORPHING_TABS_RAIL_HEIGHT - 30.0,
        );
        assert!(path.contains(inside), "the tab is enclosed");
        assert!(!path.contains(beside), "the shell beside it is not");

        // The flares are cubics, not corners: the path carries curve segments.
        let curves = path
            .elements()
            .iter()
            .filter(|e| matches!(e, kurbo::PathEl::CurveTo(..)))
            .count();
        assert_eq!(curves, 2, "one cubic flare on each side of the tab");
    }

    /// The path travels with the active tab, and clamps rather than tearing when
    /// the tab is driven past the panel's own edges.
    #[test]
    fn the_liquid_path_travels_and_clamps_at_the_panel_edges() {
        let width = 640.0;
        let tab_width = 176.0;
        let near = morphing_tabs_liquid_path(40.0, width, tab_width);
        let far = morphing_tabs_liquid_path(400.0, width, tab_width);
        assert!(
            near.contains(Point::new(100.0, MORPHING_TABS_RAIL_HEIGHT - 20.0)),
            "the near notch is over its own tab"
        );
        assert!(
            !far.contains(Point::new(100.0, MORPHING_TABS_RAIL_HEIGHT - 20.0)),
            "the far notch has left it"
        );

        // Driven past both edges, the shape stays inside the panel.
        for left in [-500.0, 5_000.0] {
            let path = morphing_tabs_liquid_path(left, width, tab_width);
            let bbox = path.bounding_box();
            assert!(bbox.x0 >= MORPHING_TABS_SURFACE_INSET - 1e-9);
            assert!(bbox.x1 <= width - MORPHING_TABS_SURFACE_INSET + 1e-9);
        }
    }

    /// The surface glides to the newly-active tab rather than snapping: strictly
    /// between the two slots mid-flight, exactly on the new one once settled.
    #[test]
    fn the_surface_glides_between_tabs() {
        let (mut w, size) = laid_out(Some("inbox"));
        settle(&mut w, size, 0.0);
        let from = w.slot_left(0);
        let to = w.slot_left(1);
        assert!((w.surface_left() - from).abs() < 0.01);

        retarget(&mut w, Some("inbox"), Some("drafts"));
        layout(&mut w);
        let (_, needs_frame) = paint_at(&mut w, size, 10_000.0);
        assert!(needs_frame, "a gliding surface owes frames");
        paint_at(&mut w, size, 10_060.0);
        let mid = w.surface_left();
        assert!(
            mid > from && mid < to,
            "mid-glide {mid} between {from} and {to}"
        );

        settle(&mut w, size, 10_100.0);
        assert!((w.surface_left() - to).abs() < 0.01);
    }

    /// A drag past the threshold displaces its neighbours, lands on the slot the
    /// midpoint rule picks, and reports the new order once.
    #[test]
    fn a_drag_reorders_by_the_midpoint_rule_and_reports_once() {
        let (mut w, size) = laid_out(Some("inbox"));
        settle(&mut w, size, 0.0);
        let mut state = App::default();
        let start = w.tab_rect(0).center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, start),
            &mut state,
        );
        assert!(w.drag.is_some() && !w.drag.as_ref().unwrap().moved);

        // Under the threshold: still not a drag.
        dispatch(
            &mut w,
            size,
            &pointer(
                PointerPhase::Move,
                Point::new(start.x + MORPHING_TABS_DRAG_THRESHOLD - 1.0, start.y),
            ),
            &mut state,
        );
        assert!(!w.drag.as_ref().unwrap().moved);

        // Past one whole slot: the tab targets slot 1 and slot 1's tab is
        // displaced back to slot 0.
        let step = w.tab_width + w.slot_gap;
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(start.x + step, start.y)),
            &mut state,
        );
        let drag = w.drag.clone().unwrap();
        assert!(drag.moved);
        assert_eq!(drag.target_index, 1);
        assert_eq!(w.visual_index(1), 0, "the passed tab moved back a slot");
        assert_eq!(w.visual_index(2), 2, "the untouched tab stayed");

        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(start.x + step, start.y)),
            &mut state,
        );
        assert_eq!(
            state.order,
            Some(vec![
                "drafts".to_string(),
                "inbox".to_string(),
                "archive".to_string()
            ])
        );
        assert_eq!(state.value_calls, 0, "a drag never activates");
        assert!(w.drag.is_none());

        // The dragged tab settles onto its landing slot rather than jumping.
        settle(&mut w, size, 1_000.0);
        assert!((w.tab_left(0) - w.slot_left(1)).abs() < 0.01);
        assert!((w.tab_left(1) - w.slot_left(0)).abs() < 0.01);
    }

    /// A press that never passes the threshold activates the tab instead, and a
    /// cancelled drag returns it home without reporting an order.
    #[test]
    fn a_short_press_activates_and_a_cancelled_drag_returns_home() {
        let (mut w, size) = laid_out(Some("inbox"));
        settle(&mut w, size, 0.0);
        let mut state = App::default();
        let at = w.tab_rect(2).center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.value, Some("archive".to_string()));
        assert_eq!(state.order, None);

        let mut state = App::default();
        let start = w.tab_rect(0).center();
        let step = w.tab_width + w.slot_gap;
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, start),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(start.x + step, start.y)),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Cancel, Point::new(start.x + step, start.y)),
            &mut state,
        );
        assert_eq!(state.order, None, "a taken-away gesture reorders nothing");
        settle(&mut w, size, 5_000.0);
        assert!((w.tab_left(0) - w.slot_left(0)).abs() < 0.01);
    }

    /// The arrows move activation (wrapping); `Alt`+arrow reorders instead, and
    /// never activates.
    #[test]
    fn the_arrows_activate_and_alt_arrows_reorder() {
        let (mut w, size) = laid_out(Some("inbox"));
        let mut state = App::default();
        dispatch(
            &mut w,
            size,
            &key_with(NamedKey::ArrowRight, false),
            &mut state,
        );
        assert_eq!(state.value, Some("drafts".to_string()));
        dispatch(
            &mut w,
            size,
            &key_with(NamedKey::ArrowLeft, false),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &key_with(NamedKey::ArrowLeft, false),
            &mut state,
        );
        assert_eq!(state.value, Some("archive".to_string()), "activation wraps");

        let mut state = App::default();
        w.focused = 0;
        dispatch(
            &mut w,
            size,
            &key_with(NamedKey::ArrowRight, true),
            &mut state,
        );
        assert_eq!(
            state.order,
            Some(vec![
                "drafts".to_string(),
                "inbox".to_string(),
                "archive".to_string()
            ])
        );
        assert_eq!(state.value_calls, 0, "a reorder never activates");
        // ...and a reorder off the end of the rail is a no-op.
        let mut state = App::default();
        w.focused = w.order[w.order.len() - 1];
        dispatch(
            &mut w,
            size,
            &key_with(NamedKey::ArrowRight, true),
            &mut state,
        );
        assert_eq!(state.order, None);
    }

    /// A disabled tab cannot be activated by press or by arrow, and cannot be
    /// reordered.
    #[test]
    fn a_disabled_tab_is_inert() {
        let make = |value: Option<&str>| {
            morphing_tabs::<App, _>(
                value.map(str::to_string),
                vec![
                    morphing_tabs_item("inbox", "Inbox", text("a")),
                    morphing_tabs_item("drafts", "Drafts", text("b")).disabled(true),
                ],
                |s: &mut App, v: String| {
                    s.value = Some(v);
                    s.value_calls += 1;
                },
            )
            .on_order_change(|s: &mut App, ids: Vec<String>| s.order = Some(ids))
        };
        let mut w = build_from(&make(Some("inbox")));
        let size = layout(&mut w);
        settle(&mut w, size, 0.0);
        let mut state = App::default();
        let at = w.tab_rect(1).center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.value_calls, 0, "a disabled tab never activates");
        assert!(w.drag.is_none(), "and never arms a drag");

        w.focused = 1;
        dispatch(
            &mut w,
            size,
            &key_with(NamedKey::ArrowRight, true),
            &mut state,
        );
        assert_eq!(state.order, None, "and never reorders");
    }

    /// The close affordance appears only with `on_close`, fires on a release
    /// back inside it, and never becomes a drag.
    #[test]
    fn the_close_affordance_fires_and_never_drags() {
        let (mut w, size) = laid_out(Some("inbox"));
        settle(&mut w, size, 0.0);
        let close = w.close_rect(w.tab_rect(0)).expect("a close box");
        let mut state = App::default();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, close.center()),
            &mut state,
        );
        assert!(w.drag.as_ref().unwrap().closing);
        dispatch(
            &mut w,
            size,
            &pointer(
                PointerPhase::Move,
                Point::new(close.center().x + 80.0, close.center().y),
            ),
            &mut state,
        );
        assert!(!w.drag.as_ref().unwrap().moved, "a close press never drags");
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, close.center()),
            &mut state,
        );
        assert_eq!(state.closed, vec!["inbox".to_string()]);
        assert_eq!(state.value_calls, 0);

        // Without `on_close` there is no box at all.
        let mut bare = build_from(&morphing_tabs::<App, _>(
            Some("inbox".into()),
            items(),
            |s: &mut App, v: String| s.value = Some(v),
        ));
        layout(&mut bare);
        assert!(bare.close_rect(bare.tab_rect(0)).is_none());
    }

    /// A switch keeps the outgoing room alive through its exit while the
    /// incoming one enters, each on its own stage sign.
    #[test]
    fn a_switch_crossfades_the_rooms() {
        let (mut w, size) = laid_out(Some("inbox"));
        settle(&mut w, size, 0.0);
        assert!(w.entries[0].presence.is_visible() && !w.entries[1].presence.is_visible());

        retarget(&mut w, Some("inbox"), Some("drafts"));
        layout(&mut w);
        paint_at(&mut w, size, 10_000.0);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, size, 10_060.0);
        assert_eq!(w.entries[0].presence.phase(), PresencePhase::Exiting);
        assert_eq!(w.entries[1].presence.phase(), PresencePhase::Entering);
        assert_eq!(rec.layers.len(), 2, "one staged layer per live room");

        let (_, exit_rise) = w.content_stage(0, ft_ms(10_060.0));
        let (_, enter_rise) = w.content_stage(1, ft_ms(10_060.0));
        assert!(exit_rise < 0.0, "the leaving room lifts out: {exit_rise}");
        assert!(enter_rise > 0.0, "the arriving room rises in: {enter_rise}");

        settle(&mut w, size, 10_200.0);
        assert!(!w.entries[0].presence.is_visible());
        assert_eq!(w.content_stage(1, ft_ms(30_000.0)), (1.0, 0.0));
    }

    /// A `value` naming no tab falls back to the first enabled one for display
    /// and reports nothing — the effect upstream runs is deliberately not ported.
    #[test]
    fn an_unmatched_value_falls_back_without_reporting() {
        let (mut w, size) = laid_out(Some("nope"));
        assert_eq!(w.active(), Some(0));
        let mut state = App::default();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(-5.0, -5.0)),
            &mut state,
        );
        assert_eq!(state.value_calls, 0, "no value is reported unasked");
    }

    /// The shell, the liquid surface and the panel all paint their resolved
    /// tokens, not upstream's four hardcoded hexes.
    #[test]
    fn every_surface_paints_its_token() {
        let (mut w, size) = laid_out(Some("inbox"));
        settle(&mut w, size, 0.0);
        let (rec, _) = paint_at(&mut w, size, 10_000.0);
        let p = crate::BEUI_LIGHT;
        assert_eq!(
            rec.rrects[0].3, p.foreground,
            "the shell is the inverted ink"
        );
        assert_eq!(rec.paths.len(), 1, "one liquid surface");
        assert_eq!(rec.paths[0].2, p.background, "filled with the panel colour");
        assert!(
            rec.rrects.iter().any(|(o, s, _, c)| *c == p.background
                && o.y == MORPHING_TABS_RAIL_HEIGHT
                && s.height == MORPHING_TABS_PANEL_MIN_HEIGHT),
            "the panel"
        );
        assert!(!rec.inks.is_empty(), "the tab labels are painted");

        let themed = crate::theme();
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(10_000.0)).with_theme(&themed);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.rrects[0].3, themed.scheme().on_surface);
    }

    /// `reduce_motion` collapses every ramp: the surface is on the new tab on
    /// the first frame and nothing owes a frame.
    #[test]
    fn reduced_motion_lands_the_glide_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(Some("inbox"));
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);

        retarget(&mut w, Some("inbox"), Some("archive"));
        layout(&mut w);
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(16.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(!ctx.needs_frame(), "a collapsed ramp owes no frame");
        assert!((w.surface_left() - w.slot_left(2)).abs() < 0.01);
    }

    /// The rail publishes its tabs in the rail's own order, with the open one
    /// selected and a close button per tab when one is wired.
    #[test]
    fn semantics_publish_the_rail_in_order() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = App::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut App| view(Some("drafts"));
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(640.0, 900.0), &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3);
        assert_eq!(
            tabs.iter()
                .filter(|(_, n)| n.is_selected() == Some(true))
                .count(),
            1
        );
        assert_eq!(
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == Role::Button)
                .count(),
            3,
            "one close button per tab"
        );
    }

    // ---- Typeface: the tab labels follow the live theme ---------------------

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(600.0, 400.0);

    /// Two tabs with glyph-free rooms, the first open.
    fn probe_view(_: &mut ()) -> MorphingTabsView<()> {
        let room = || frust::SizedBox::<()>(Some(40.0), Some(40.0));
        morphing_tabs::<(), _>(
            Some("inbox".to_string()),
            vec![
                morphing_tabs_item("inbox", "Inbox", room()),
                morphing_tabs_item("drafts", "Drafts", room()),
            ],
            |_: &mut (), _| {},
        )
    }

    #[test]
    fn the_tab_labels_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the tab labels", probe_view, PROBE_WINDOW);
        let runs = Probe::new(probe_view, PROBE_WINDOW, crate::theme()).frame();
        assert_eq!(runs.len(), 2, "one label per tab");
    }

    #[test]
    fn the_tab_labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the tab labels", probe_view, PROBE_WINDOW);
    }
}
