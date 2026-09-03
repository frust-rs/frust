//! The modal overlay host: a full-area layer that paints the scrim, mounts one
//! panel against a centre or an edge, and stages that panel's entrance and exit.
//!
//! # What upstream shares between its four modal components
//!
//! `morphing-modal.tsx`, `center-morph-modal.tsx`, `drawer.tsx` and
//! `bottom-sheet.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01) are four
//! different panels over one hosting pattern: a `fixed inset-0` backdrop that
//! fades on its own tween, a `fixed` panel pinned to the centre or to one edge,
//! an Escape key listener, a backdrop click that closes, and `body { overflow:
//! hidden }` while up. This host is that pattern; the panels themselves are
//! their own components.
//!
//! The three chrome-shaped values it keeps from them:
//!
//! * **Centre** — `w-full max-w-sm` inside `inset-4`
//!   ([`PANEL_MAX_WIDTH`], [`PANEL_MARGIN`]), entering `{opacity: 0, y: 20,
//!   scale: 0.97}` on `SPRING_PANEL` and leaving to `scale: 0.98` over 180ms
//!   ([`PANEL_ENTER_LIFT`], [`PANEL_ENTER_SCALE`], [`PANEL_EXIT_SCALE`],
//!   [`PANEL_EXIT`]).
//! * **Side edge** — `w-80 max-w-[85vw]`, full height, sliding its own width off
//!   the edge ([`DRAWER_WIDTH`], [`DRAWER_WIDTH_FRACTION`]).
//! * **Bottom edge** — the full width at half the viewport height
//!   ([`SHEET_HEIGHT_FRACTION`], the first of the sheet's own `[0.5, 0.92]` snap
//!   points), sliding its own height down.
//!
//! There is no `overflow: hidden` equivalent to keep: nothing scrolls behind a
//! frust modal, because the host swallows every event its panel declined.
//!
//! # The host paints the scrim, the panel paints itself
//!
//! The seam owns what every modal shares — the scrim, the mount point, the
//! ramps, the dismissal, the semantics container — and nothing that differs. A
//! panel's fill, border, corner radius, shadow, close button and drag handle
//! belong to the component mounted as `content`, which is why this module has no
//! chrome vocabulary at all. A component wanting the frosted backdrop
//! `morphing-modal.tsx` uses instead of the dimming one turns
//! [`ModalConfig::scrim`] off and paints its own.
//!
//! # Two ramps, one dismissal
//!
//! The scrim and the panel are staged by **two** [`Presence`] drivers, because
//! upstream times them differently: the backdrop is a plain
//! [`SCRIM_FADE`]-long ease while the panel is on `SPRING_PANEL`. They are
//! opened and closed together and can never disagree about whether the modal is
//! present; a staged close waits for *both* to settle.
//!
//! # Hit testing is against the resting rect
//!
//! Both ramps are scene transforms, not layout: the panel's pod stays where
//! layout put it, so a press mid-ramp is tested against the panel's *resting*
//! rect rather than the place it is currently drawn. For a centred panel that
//! is a [`PANEL_ENTER_LIFT`] offset and a few percent of scale; for an edge
//! mount it means a drawer accepts presses in its resting column while it is
//! still sliding in. Re-laying the panel out every frame to keep the two in
//! step is exactly the cost the transform exists to avoid, and a press landing
//! during a ramp shorter than 200ms is not a gesture worth paying it for.
//!
//! # Dismissal, and the navigator seam
//!
//! - **Escape**, and **every other key**: Escape dismisses a
//!   [`dismissable`](ModalConfig::dismissable) modal; any other key the content
//!   declined is swallowed, because nothing behind a modal may act on a
//!   keystroke aimed at it. The host claims focus on every `Down`, which is what
//!   makes Escape reachable at all.
//! - **A backdrop click** — press *and* release outside the panel, matching
//!   upstream's backdrop being a `<button onClick>` rather than a pointerdown
//!   listener (which is what the anchored host's light dismiss ports instead).
//!   Every press is swallowed either way: the host is a barrier.
//! - **A back press**, when pushed by [`show_modal`]: the page carries
//!   [`BackPolicy::DismissAnimated`], so the navigator bumps the shared
//!   dismiss-signal cell instead of popping, and this widget stages the same
//!   exit a backdrop click would — the panel animates out and *then* pops. A
//!   non-dismissable modal is pushed [`BackPolicy::Veto`] instead: the back
//!   press is consumed and does nothing.
//!
//! Which callback a dismissal reaches depends on the mount:
//!
//! * [`ModalView::on_dismiss`] — state-bearing, fired from the event pass the
//!   moment the user asks to close. A `Stack`-mounted app flips its own open
//!   flag here, and the exit starts on the next rebuild.
//! * [`ModalView::on_close`] — state-free, fired from the paint that settles the
//!   exit. This is the navigator path: [`show_modal`] installs a guarded
//!   `controller.pop()`, so the pop lands *after* the exit rather than
//!   truncating it. A `Down` on a paint pass has no `&mut State` to reach, which
//!   is why this half is state-free — and why the exit-completion latch is
//!   drained here rather than one event later, the deferral
//!   [`crate::motion::presence`] documents for the state-bearing case.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, Role, SemanticsCtx, Size, Vec2, View, Widget, any, build_child, erase_callback,
    rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{BackPolicy, NavigatorController, PopResult, PushOptions, Theme, TransitionSpec};

use super::finite_or_zero;
use crate::motion::{Presence, PresencePhase, Ramp};
use crate::style;
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};

/// `max-w-sm` — a centred panel's width cap, in logical px.
pub const PANEL_MAX_WIDTH: f64 = 384.0;

/// `inset-4` — the margin a centred panel keeps from every edge of the area, in
/// logical px.
pub const PANEL_MARGIN: f64 = 16.0;

/// The scale a centred panel enters from (`morphing-modal.tsx`'s `enterScale`).
pub const PANEL_ENTER_SCALE: f64 = 0.97;

/// The scale a centred panel leaves to (`morphing-modal.tsx`'s `exit.scale`) —
/// shallower than the entrance, so leaving reads quicker than arriving.
pub const PANEL_EXIT_SCALE: f64 = 0.98;

/// How far below its resting position a centred panel starts, in logical px
/// (`morphing-modal.tsx`'s `enterY` for the centred placement; its bottom
/// placement doubles this, which an edge mount expresses as a slide instead).
pub const PANEL_ENTER_LIFT: f64 = 20.0;

/// How long a panel's exit takes (`morphing-modal.tsx`'s
/// `exit.transition.duration`).
pub const PANEL_EXIT: Duration = Duration::from_millis(180);

/// How long the scrim takes to fade, in or out (`morphing-modal.tsx`'s backdrop
/// transition; `drawer.tsx` gives its own backdrop 250ms, which a caller
/// restates through [`ModalConfig::scrim_fade`]).
pub const SCRIM_FADE: Duration = Duration::from_millis(200);

/// `w-80` — a side drawer's width, in logical px.
pub const DRAWER_WIDTH: f64 = 320.0;

/// `max-w-[85vw]` — the share of the area a side drawer may not exceed.
pub const DRAWER_WIDTH_FRACTION: f64 = 0.85;

/// The share of the area a bottom sheet takes at rest — the first of
/// `bottom-sheet.tsx`'s own `[0.5, 0.92]` snap points, which is its
/// `defaultSnap`. The drag-between-snap-points gesture is the sheet component's,
/// not the host's.
pub const SHEET_HEIGHT_FRACTION: f64 = 0.5;

/// Progress difference below which a ramp counts as settled for the staged
/// close.
const PROGRESS_EPSILON: f64 = 1e-3;

/// Standard deviations of Gaussian blur beyond which a drop shadow's own
/// visible contribution is negligible — three sigma covers ~99.7% of it.
const SHADOW_SPILL_SIGMAS: f64 = 3.0;

/// `tokens::theme`'s `glass_scale` chrome tier's own `ShadowSpec` — the recipe
/// a panel painting the glass chrome shadow (`y_offset 24, blur_std_dev 30`)
/// actually casts. [`SHADOW_SPILL_NEAR`]/[`SHADOW_SPILL_FAR`] are derived from
/// these two numbers rather than the live `ShadowSpec`, because the theme
/// builds it inside a closure with no standalone constant to import.
const GLASS_SHADOW_Y_OFFSET: f64 = 24.0;
const GLASS_SHADOW_BLUR_STD_DEV: f64 = 30.0;

/// How far outside the panel its own content shadow may reach on the sides
/// with no directional offset (top, left, right): [`SHADOW_SPILL_SIGMAS`]
/// standard deviations of the chrome shadow's own blur — mirroring
/// `plugins/shadcn/src/overlay/modal.rs`'s `SHADOW_SPILL`, split in two here
/// because that shadow's `y_offset` makes only one side of it asymmetric (see
/// [`SHADOW_SPILL_FAR`]).
const SHADOW_SPILL_NEAR: f64 = SHADOW_SPILL_SIGMAS * GLASS_SHADOW_BLUR_STD_DEV;

/// How far outside the panel its shadow reaches on the offset (downward)
/// side: the same [`SHADOW_SPILL_SIGMAS`] standard deviations of blur, plus
/// the shadow's own `y_offset`.
const SHADOW_SPILL_FAR: f64 =
    GLASS_SHADOW_Y_OFFSET + SHADOW_SPILL_SIGMAS * GLASS_SHADOW_BLUR_STD_DEV;

/// Which edge an edge-mounted panel is pinned to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalEdge {
    /// The leading edge — a left drawer.
    Left,
    /// The trailing edge — `drawer.tsx`'s own default.
    Right,
    /// The top edge — a notification shade.
    Top,
    /// The bottom edge — a bottom sheet.
    Bottom,
}

impl ModalEdge {
    /// The direction a panel slides in from, as a unit vector in the host's own
    /// space: away from the edge it is pinned to.
    const fn offscreen(self) -> Vec2 {
        match self {
            ModalEdge::Left => Vec2::new(-1.0, 0.0),
            ModalEdge::Right => Vec2::new(1.0, 0.0),
            ModalEdge::Top => Vec2::new(0.0, -1.0),
            ModalEdge::Bottom => Vec2::new(0.0, 1.0),
        }
    }

    /// Whether the panel slides along the vertical axis.
    pub const fn is_vertical(self) -> bool {
        matches!(self, ModalEdge::Top | ModalEdge::Bottom)
    }
}

/// Where the panel sits inside the host area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalMount {
    /// Centred on both axes, entering with a lift, a scale and a fade — the
    /// dialog/command-palette shape.
    #[default]
    Center,
    /// Pinned to one edge and filling the cross axis, sliding in off that edge —
    /// the drawer/bottom-sheet shape.
    Edge(ModalEdge),
}

/// How much of an axis the panel takes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ModalExtent {
    /// Hug the content, up to the axis' limit — a dialog's height.
    #[default]
    Hug,
    /// A fraction of the area's own extent on this axis — a bottom sheet's
    /// height, a drawer's full height.
    Fraction(f64),
    /// A fixed logical-px extent — a drawer's `w-80`.
    Fixed(f64),
}

/// An upper bound on a resolved [`ModalExtent`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ModalLimit {
    /// No cap beyond the area itself.
    #[default]
    None,
    /// A cap in logical px — `max-w-sm`.
    Px(f64),
    /// A cap as a fraction of the area's extent — `max-w-[85vw]`.
    Fraction(f64),
}

impl ModalLimit {
    /// This limit against an area extent of `available`.
    fn resolve(self, available: f64) -> f64 {
        match self {
            ModalLimit::None => available,
            ModalLimit::Px(px) => px.min(available),
            ModalLimit::Fraction(f) => (available * f).min(available),
        }
    }
}

/// The accessibility role the modal reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalRole {
    /// A dialog box (`Role::Dialog`) — every modal upstream ships.
    #[default]
    Dialog,
    /// An alert dialog demanding a response (`Role::AlertDialog`), for a
    /// component that builds one out of this host.
    AlertDialog,
}

impl ModalRole {
    /// The accesskit role.
    const fn role(self) -> Role {
        match self {
            ModalRole::Dialog => Role::Dialog,
            ModalRole::AlertDialog => Role::AlertDialog,
        }
    }
}

/// The shape of one modal: where its panel mounts, how big it is, whether it
/// dismisses, and what it is timed by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModalConfig {
    /// Where the panel sits.
    pub mount: ModalMount,
    /// The panel's width rule.
    pub width: ModalExtent,
    /// The cap on the resolved width.
    pub max_width: ModalLimit,
    /// The panel's height rule.
    pub height: ModalExtent,
    /// The cap on the resolved height.
    pub max_height: ModalLimit,
    /// The margin kept from every edge of the area, in logical px.
    pub margin: f64,
    /// Whether the host paints [`super::scrim`] behind the panel. A panel that
    /// paints its own backdrop turns this off; the host stays a barrier either
    /// way.
    pub scrim: bool,
    /// Whether Escape, a backdrop click and a back press dismiss it.
    pub dismissable: bool,
    /// The accessibility role.
    pub role: ModalRole,
    /// The panel's entrance ramp.
    pub enter: Ramp,
    /// The panel's exit ramp.
    pub exit: Ramp,
    /// How long the scrim takes to fade, each way.
    pub scrim_fade: Duration,
}

impl Default for ModalConfig {
    fn default() -> Self {
        Self::centered()
    }
}

impl ModalConfig {
    /// A centred panel: the full width up to [`PANEL_MAX_WIDTH`], hugging its
    /// content vertically, inside a [`PANEL_MARGIN`] inset — `morphing-modal`'s
    /// own `w-full max-w-sm` within `inset-4`.
    pub const fn centered() -> Self {
        ModalConfig {
            mount: ModalMount::Center,
            width: ModalExtent::Fraction(1.0),
            max_width: ModalLimit::Px(PANEL_MAX_WIDTH),
            height: ModalExtent::Hug,
            max_height: ModalLimit::None,
            margin: PANEL_MARGIN,
            scrim: true,
            dismissable: true,
            role: ModalRole::Dialog,
            enter: Ramp::spring(SPRING_PANEL),
            exit: Ramp::eased(PANEL_EXIT, EASE_OUT),
            scrim_fade: SCRIM_FADE,
        }
    }

    /// A side drawer on `edge`: [`DRAWER_WIDTH`] capped at
    /// [`DRAWER_WIDTH_FRACTION`] of the area, full height, flush to the edge —
    /// `drawer.tsx`'s `inset-y-0 w-80 max-w-[85vw]`.
    ///
    /// A vertical `edge` gets the same treatment on the other axis: the full
    /// width, [`DRAWER_WIDTH`] tall. [`sheet`](Self::sheet) is the usual bottom
    /// mount.
    pub const fn drawer(edge: ModalEdge) -> Self {
        let across = ModalExtent::Fraction(1.0);
        let along = ModalExtent::Fixed(DRAWER_WIDTH);
        let along_limit = ModalLimit::Fraction(DRAWER_WIDTH_FRACTION);
        let vertical = edge.is_vertical();
        ModalConfig {
            mount: ModalMount::Edge(edge),
            width: if vertical { across } else { along },
            max_width: if vertical {
                ModalLimit::None
            } else {
                along_limit
            },
            height: if vertical { along } else { across },
            max_height: if vertical {
                along_limit
            } else {
                ModalLimit::None
            },
            margin: 0.0,
            ..Self::centered()
        }
    }

    /// A bottom sheet: the full width at [`SHEET_HEIGHT_FRACTION`] of the area's
    /// height, flush to the bottom edge.
    pub const fn sheet() -> Self {
        ModalConfig {
            mount: ModalMount::Edge(ModalEdge::Bottom),
            width: ModalExtent::Fraction(1.0),
            max_width: ModalLimit::None,
            height: ModalExtent::Fraction(SHEET_HEIGHT_FRACTION),
            max_height: ModalLimit::None,
            margin: 0.0,
            ..Self::centered()
        }
    }

    /// Set the width rule and its cap.
    pub const fn width(mut self, width: ModalExtent, max_width: ModalLimit) -> Self {
        self.width = width;
        self.max_width = max_width;
        self
    }

    /// Set the height rule and its cap.
    pub const fn height(mut self, height: ModalExtent, max_height: ModalLimit) -> Self {
        self.height = height;
        self.max_height = max_height;
        self
    }

    /// Set the margin kept from the area's edges, in logical px.
    pub const fn margin(mut self, margin: f64) -> Self {
        self.margin = margin;
        self
    }

    /// Turn the host's own scrim on or off.
    pub const fn scrim(mut self, scrim: bool) -> Self {
        self.scrim = scrim;
        self
    }

    /// Make the modal dismissable, or not.
    pub const fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the accessibility role.
    pub const fn role(mut self, role: ModalRole) -> Self {
        self.role = role;
        self
    }

    /// Replace the panel's entrance and exit ramps.
    pub const fn ramps(mut self, enter: Ramp, exit: Ramp) -> Self {
        self.enter = enter;
        self.exit = exit;
        self
    }

    /// Set how long the scrim takes to fade, each way.
    pub const fn scrim_fade(mut self, scrim_fade: Duration) -> Self {
        self.scrim_fade = scrim_fade;
        self
    }
}

/// Resolve one axis of the panel: the constraint pair to lay the content out
/// with, given the area extent already reduced by the margins.
fn axis(extent: ModalExtent, limit: ModalLimit, available: f64) -> (f64, f64) {
    let cap = limit.resolve(available).max(0.0);
    match extent {
        ModalExtent::Hug => (0.0, cap),
        ModalExtent::Fraction(f) => {
            let v = (available * f).min(cap).max(0.0);
            (v, v)
        }
        ModalExtent::Fixed(px) => {
            let v = px.min(cap).max(0.0);
            (v, v)
        }
    }
}

/// The constraints a `config`-shaped panel lays its content out with inside an
/// `area`-sized host.
fn panel_constraints(config: ModalConfig, area: Size) -> BoxConstraints {
    let inset = config.margin * 2.0;
    let available = Size::new(
        (area.width - inset).max(0.0),
        (area.height - inset).max(0.0),
    );
    let (min_w, max_w) = axis(config.width, config.max_width, available.width);
    let (min_h, max_h) = axis(config.height, config.max_height, available.height);
    BoxConstraints::new(Size::new(min_w, min_h), Size::new(max_w, max_h))
}

/// Where a `panel`-sized panel sits inside an `area`-sized host.
///
/// Centred on both axes for [`ModalMount::Center`]; flush against the named edge
/// (inside the margin) and centred on the cross axis for
/// [`ModalMount::Edge`].
fn panel_rect(config: ModalConfig, area: Size, panel: Size) -> Rect {
    let centre_x = (area.width - panel.width) / 2.0;
    let centre_y = (area.height - panel.height) / 2.0;
    let m = config.margin;
    let origin = match config.mount {
        ModalMount::Center => Point::new(centre_x, centre_y),
        ModalMount::Edge(ModalEdge::Left) => Point::new(m, centre_y),
        ModalMount::Edge(ModalEdge::Right) => Point::new(area.width - panel.width - m, centre_y),
        ModalMount::Edge(ModalEdge::Top) => Point::new(centre_x, m),
        ModalMount::Edge(ModalEdge::Bottom) => Point::new(centre_x, area.height - panel.height - m),
    };
    Rect::from_origin_size(origin, panel)
}

/// A guarded, state-free close: the [`ModalView::on_close`] payload, fired from
/// the paint that settles the exit.
///
/// Staging puts a whole exit ramp between the *decision* to dismiss and the
/// [`NavigatorController::pop`] that carries it out, and a pop always takes the
/// **top** page. Anything pushed inside that window (a deep link, an async
/// completion landing) would otherwise be popped instead of this modal.
/// Arming it snapshots the stack depth the exit was staged against, and firing
/// it refuses the pop if that depth moved — reporting the refusal, so the widget
/// does not latch itself closed over a page it never removed.
///
/// The snapshot is advisory: `depth` publishes at the last rebuild, so a push
/// enqueued after the rebuild preceding the settling paint is still invisible.
/// That narrows the exposure from the ramp's whole length to a single frame; it
/// does not erase it.
///
/// Public only because it names the payload; build one with
/// [`ModalView::on_close`], or let [`show_modal`] install the navigator's.
pub struct StagedPop {
    /// Reads the navigator's live page-stack depth — `None` for a hook with no
    /// navigator behind it, whose close is always authoritative.
    depth: Option<Box<dyn Fn() -> usize>>,
    /// The close itself.
    close: Box<dyn Fn()>,
    /// The depth [`arm`](Self::arm) last snapshotted; `None` until it runs.
    staged_depth: Cell<Option<usize>>,
}

impl StagedPop {
    /// A navigator pop, guarded by the stack identity above.
    fn navigator_pop<State: 'static>(controller: &NavigatorController<State>) -> Self {
        let depth_ctrl = controller.clone();
        let pop_ctrl = controller.clone();
        StagedPop {
            depth: Some(Box::new(move || depth_ctrl.depth())),
            close: Box::new(move || pop_ctrl.pop()),
            staged_depth: Cell::new(None),
        }
    }

    /// A close with no navigator identity to check — an app's own hook on a
    /// `Stack` mount, which may be a signal write rather than a pop at all and
    /// is not this module's to second-guess.
    fn unguarded<F: Fn() + 'static>(close: F) -> Self {
        StagedPop {
            depth: None,
            close: Box::new(close),
            staged_depth: Cell::new(None),
        }
    }

    /// Snapshot the stack this exit is being staged against.
    fn arm(&self) {
        self.staged_depth.set(self.depth.as_ref().map(|d| d()));
    }

    /// Fire the close, unless the stack moved under the staged exit. Returns
    /// whether it fired.
    fn fire(&self) -> bool {
        if let (Some(depth), Some(staged)) = (&self.depth, self.staged_depth.get())
            && depth() != staged
        {
            return false;
        }
        (self.close)();
        true
    }
}

/// The state-free close hook a modal fires when its exit settles.
pub type OnClose = Rc<StagedPop>;

/// A view-held, typed callback (erased on build).
type OnState<State> = Rc<dyn Fn(&mut State)>;

/// A modal component [`show_modal`] can wire a navigator pop into.
///
/// Every modal component in the catalog implements it by wrapping a
/// [`ModalView`]; the trait exists so one push helper serves all of them rather
/// than each re-deriving the same `push_with_options` call. Its element is
/// [`ModalWidget`] by definition — a modal component *is* a pre-configured modal
/// host — which is what lets [`show_modal`] install the staged close and the
/// back-press signal on whatever component it was handed, with no per-component
/// setter.
pub trait ModalContent<State: 'static>: View<State, Element = ModalWidget> {
    /// Whether this component dismisses on Escape, a backdrop click and a back
    /// press. Peeked once, at push time, to pick the page's [`BackPolicy`].
    fn modal_dismissable(&self) -> bool;
}

/// Build a modal host showing `content`, shaped by `config`.
///
/// The content view owns the panel entirely — its fill, border, corners, shadow
/// and padding. The host contributes the scrim, the mount point, the ramps and
/// the dismissal.
pub fn modal<State: 'static, V: View<State>>(content: V, config: ModalConfig) -> ModalView<State> {
    ModalView {
        content: any(content),
        config,
        label: None,
        open: true,
        on_dismiss: None,
        on_close: None,
    }
}

/// A declarative modal host. See [`modal`].
///
/// The fields are crate-visible so the catalog's modal components can wrap one
/// and adjust its config in their own builders; an app outside the crate
/// configures it through [`modal`]'s `config` argument and the setters below.
pub struct ModalView<State: 'static> {
    pub(crate) content: AnyView<State>,
    pub(crate) config: ModalConfig,
    pub(crate) label: Option<String>,
    pub(crate) open: bool,
    pub(crate) on_dismiss: Option<OnState<State>>,
    pub(crate) on_close: Option<OnClose>,
}

impl<State: 'static> ModalView<State> {
    /// Tell a kept-mounted host whether it is open. The default is `true`: a
    /// mounted host is an open one, which is the shape [`show_modal`] pushes
    /// (the page's own lifetime is the flag).
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// The accessibility label the panel is announced with.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Replace the config wholesale.
    pub fn config(mut self, config: ModalConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the dismiss-requested callback: Escape, a backdrop click, or a back
    /// press. Fired from the event pass, once per dismissal (see the [module
    /// docs](self)).
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Set the exit-settled callback: state-free, fired from the paint that
    /// finishes the exit ramp a dismissal staged.
    ///
    /// Installing one is what makes a dismissal *stage* its own exit rather than
    /// only reporting itself — the navigator path, where there is no app flag to
    /// flip. [`show_modal`] installs a guarded `controller.pop()` here.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(StagedPop::unguarded(on_close)));
        self
    }
}

impl<State: 'static> ModalContent<State> for ModalView<State> {
    fn modal_dismissable(&self) -> bool {
        self.config.dismissable
    }
}

/// The retained widget for a [`ModalView`].
pub struct ModalWidget {
    content: ChildPod,
    config: ModalConfig,
    label: Option<String>,
    /// Whether the modal is open. `false` makes the host inert without
    /// unmounting it — the kept-mounted pattern.
    open: bool,
    /// The panel's own driver (spring in, eased out).
    panel: Presence,
    /// The scrim's driver — a plain fade on its own duration, opened and closed
    /// with the panel.
    scrim: Presence,
    /// The `reduce_motion` value the drivers were last built for; `None` until
    /// the first paint resolves a theme.
    reduced: Option<bool>,
    on_dismiss: Option<ErasedCallback>,
    on_close: Option<OnClose>,
    /// The shared back-press dismiss-signal cell, installed by [`show_modal`].
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last generation observed from `dismiss_signal`.
    last_seen_dismiss: u64,
    /// Whether a dismissal has staged this host's own exit, so the close fires
    /// when it settles.
    staged: bool,
    /// Whether the staged close already fired — the guard that makes a
    /// dismissal fire exactly once.
    closed: bool,
    /// The panel rect, in this host's own coordinate space.
    rect: Rect,
    /// Whether a press is being tracked for a backdrop click, and whether it
    /// started outside the panel.
    scrim_captured: bool,
    scrim_down_outside: bool,
    /// A focus release a dismissal could not deliver because the dispatch
    /// that requested it was a pointer event (see [`Self::request_dismiss`]),
    /// drained on this host's next `Scroll`/`Key`/`Ime` pass.
    pending_focus_release: bool,
}

impl ModalWidget {
    /// The panel rect in the host's own coordinate space.
    pub fn panel_rect(&self) -> Rect {
        self.rect
    }

    /// Whether the host is open (see [`ModalView::open`]).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The panel driver's current phase.
    pub fn phase(&self) -> PresencePhase {
        self.panel.phase()
    }

    /// Install the staged close hook — the seam [`show_modal`] reaches a
    /// component's inner host through.
    pub fn set_on_close(&mut self, on_close: Option<OnClose>) {
        self.on_close = on_close;
    }

    /// Install the shared back-press dismiss-signal cell, seeding the observed
    /// generation so the cell's current value never reads as a fresh bump.
    pub fn set_dismiss_signal(&mut self, signal: Option<Rc<Cell<u64>>>) {
        self.last_seen_dismiss = signal.as_ref().map_or(0, |s| s.get());
        self.dismiss_signal = signal;
    }

    /// Rebuild both drivers when `reduce_motion` flips, preserving what the old
    /// ones were doing.
    ///
    /// [`Presence::collapsed`] is a constructor, not a switch, so a live toggle
    /// has to replace the drivers. One replaced mid-*exit* is re-opened and
    /// re-closed on the spot, which keeps a staged close reachable instead of
    /// stranding the modal at zero presence over a page it never popped.
    fn sync_motion(&mut self, reduce: bool) {
        if self.reduced == Some(reduce) {
            return;
        }
        let was_exiting = self.panel.phase() == PresencePhase::Exiting;
        let closing = was_exiting || (self.staged && !self.closed);
        self.reduced = Some(reduce);
        let panel = Presence::new(self.config.enter, self.config.exit);
        let fade = Ramp::eased(self.config.scrim_fade, EASE_OUT);
        let scrim = Presence::symmetric(fade);
        let (mut panel, mut scrim) = if reduce {
            (panel.collapsed(), scrim.collapsed())
        } else {
            (panel, scrim)
        };
        let open = self.open && !closing;
        if closing {
            panel.set_open(true);
            scrim.set_open(true);
        }
        panel.set_open(open);
        scrim.set_open(open);
        self.panel = panel;
        self.scrim = scrim;
    }

    /// Whether a dismissal may still be started.
    fn dismissable(&self) -> bool {
        self.config.dismissable && !self.staged && !self.closed && self.open
    }

    /// Start a dismissal: report it to the app, and stage the exit when a
    /// state-free close is installed to fire at the end of it.
    ///
    /// Refuses a second one — that is what makes each dismiss path fire its
    /// callbacks exactly once.
    fn request_dismiss(&mut self, ctx: &mut EventCtx, event: &InputEvent) {
        if !self.dismissable() {
            return;
        }
        if let Some(on_dismiss) = self.on_dismiss.as_mut() {
            on_dismiss(ctx);
        }
        if let Some(on_close) = &self.on_close {
            on_close.arm();
            self.staged = true;
            self.panel.set_open(false);
            self.scrim.set_open(false);
            ctx.request_redraw();
        }
        if ctx.has_focus() {
            // Hand the keyboard chain back: the panel is leaving, and nothing
            // inside it should keep the focus path. `EventCtx::release_focus`
            // only reaches the root's own focus session on a `Scroll`/`Key`/
            // `Ime` dispatch (the root's event loop never consults a release
            // on the `Pointer` arm), so a dismissal reached from the backdrop
            // Up is deferred instead of dropped — [`Self::drain_focus_release`]
            // fires it on this host's next pass of one of those kinds.
            if matches!(
                event,
                InputEvent::Scroll { .. } | InputEvent::Key(_) | InputEvent::Ime(_)
            ) {
                ctx.release_focus();
            } else {
                self.pending_focus_release = true;
            }
        }
    }

    /// Deliver a focus release [`Self::request_dismiss`] could not make stick
    /// on the pointer dispatch that requested it, the moment a dispatch of a
    /// kind the root's own focus bookkeeping actually honours comes through.
    fn drain_focus_release(&mut self, ctx: &mut EventCtx, event: &InputEvent) {
        if !self.pending_focus_release
            || !matches!(
                event,
                InputEvent::Scroll { .. } | InputEvent::Key(_) | InputEvent::Ime(_)
            )
        {
            return;
        }
        self.pending_focus_release = false;
        if ctx.has_focus() {
            ctx.release_focus();
        }
    }

    /// Observe the shared back-press dismiss-signal cell and stage the exit
    /// exactly once per bump — the [`BackPolicy::DismissAnimated`] seam's
    /// widget-side half.
    ///
    /// The paint-pass twin of [`Self::request_dismiss`], with two differences a
    /// paint forces: the state-bearing `on_dismiss` cannot be reached (no `&mut
    /// State` exists here), and the frame whose rebuild drains the enqueued pop
    /// has to be asked for explicitly.
    fn observe_dismiss_signal(&mut self, ctx: &mut PaintCtx) {
        let Some(signal) = &self.dismiss_signal else {
            return;
        };
        let current = signal.get();
        if current == self.last_seen_dismiss {
            return;
        }
        self.last_seen_dismiss = current;
        if !self.dismissable() {
            return;
        }
        if let Some(on_close) = &self.on_close {
            on_close.arm();
            self.staged = true;
            self.panel.set_open(false);
            self.scrim.set_open(false);
            ctx.request_frame();
        }
    }

    /// Fire a staged close once both ramps have settled at zero.
    ///
    /// The hook only *enqueues* a navigator pop (it writes no tracked signal),
    /// so this paint asks for the frame whose rebuild drains it — otherwise a
    /// `ControlFlow::Wait` desktop shell idles and the modal never leaves. A
    /// [`StagedPop`] that refuses (the stack moved under it) leaves `closed`
    /// false, so the next settled paint tries again rather than latching the
    /// host closed over a page it never removed.
    fn fire_staged_close(&mut self, ctx: &mut PaintCtx) {
        if !self.staged || self.closed {
            return;
        }
        if self.panel.is_animating() || self.scrim.is_animating() {
            return;
        }
        if self.panel.presence(ctx.frame_time()) > PROGRESS_EPSILON {
            return;
        }
        self.panel.take_exited();
        let Some(on_close) = &self.on_close else {
            return;
        };
        if on_close.fire() {
            self.closed = true;
            ctx.request_frame();
        } else {
            // The stack moved under the staged pop, so it was refused (see
            // [`StagedPop::fire`]). Left `staged` set, the host would latch
            // itself un-dismissable — every later dismissal needs it clear —
            // over a fully exited, input-transparent panel that no longer
            // swallows anything and a page it never actually popped.
            //
            // Recoverable, not bricked, but only when the app still considers
            // the host open: reopening the panel and the scrim unconditionally
            // would resurrect a visible, input-swallowing, undismissable,
            // unannounced barrier over a page whose own flag already says it
            // should be gone (`ModalView::open(false)`, never resynced because
            // the refusal short-circuited before `on_dismiss` fired). `open`
            // reflects the app's own intent regardless of the refusal, so it
            // is what decides whether there is anything to reopen — a closed
            // app flag just clears the latch and leaves the panel to finish
            // settling absent, the same as any other close.
            self.staged = false;
            if self.open {
                self.panel.set_open(true);
                self.scrim.set_open(true);
                ctx.request_frame();
            }
        }
    }
}

impl<State: 'static> View<State> for ModalView<State> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        let mut panel = Presence::new(self.config.enter, self.config.exit);
        let mut scrim = Presence::symmetric(Ramp::eased(self.config.scrim_fade, EASE_OUT));
        panel.set_open(self.open);
        scrim.set_open(self.open);
        ModalWidget {
            content: build_child(&self.content, ctx),
            config: self.config,
            label: self.label.clone(),
            open: self.open,
            panel,
            scrim,
            reduced: None,
            on_dismiss: self.on_dismiss.as_ref().map(erase_callback),
            on_close: self.on_close.clone(),
            dismiss_signal: None,
            last_seen_dismiss: 0,
            staged: false,
            closed: false,
            rect: Rect::ZERO,
            scrim_captured: false,
            scrim_down_outside: false,
            pending_focus_release: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.config != self.config {
            element.config = self.config;
            // Force the next paint to rebuild the drivers around the new ramps,
            // the same path a `reduce_motion` flip takes.
            element.reduced = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            element.panel.set_open(self.open);
            element.scrim.set_open(self.open);
            if self.open {
                // The latches describe **one** open episode, not the widget's
                // whole lifetime: a transition back into an open phase — after
                // a prior exit settled (`closed`), or interrupting one still
                // animating (`staged`) — starts a fresh episode. Left set, a
                // reopened panel would stay permanently un-dismissable
                // (`dismissable` requires both clear) and unannounced
                // (`semantics` returns early on `staged`), latched over a panel
                // that is visibly back and interactive.
                element.staged = false;
                element.closed = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        // Closures aren't comparable, so the dismiss adapter is reinstalled
        // unconditionally. `on_close` is only overwritten when the view carries
        // one, so a hook installed on the widget by `show_modal` survives every
        // later rebuild of the component it wraps.
        element.on_dismiss = self.on_dismiss.as_ref().map(erase_callback);
        if self.on_close.is_some() {
            element.on_close = self.on_close.clone();
        }
        flags
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for ModalWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        let panel = self
            .content
            .layout_child(ctx, &panel_constraints(self.config, area));
        self.rect = panel_rect(self.config, area, panel);
        self.content.set_origin(self.rect.origin());
        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|theme| theme.motion.reduce_motion);
        self.sync_motion(reduce);
        self.observe_dismiss_signal(ctx);

        let now = ctx.frame_time();
        let progress = self.panel.advance(now);
        let veil = self.scrim.advance(now).clamp(0.0, 1.0);
        if self.panel.is_animating() || self.scrim.is_animating() {
            ctx.request_frame();
        }
        self.fire_staged_close(ctx);

        if self.scrim.is_visible() && self.config.scrim {
            let color = super::scrim(Theme::from_paint_ctx(ctx));
            scene.fill_rect(
                ctx.origin(),
                ctx.size(),
                style::scale_alpha(color, veil as f32),
            );
        }
        if !self.panel.is_visible() {
            return;
        }

        let rect = self.rect + ctx.origin().to_vec2();
        let mut layer_pushed = false;
        match self.config.mount {
            ModalMount::Center => {
                // `{opacity, y, scale}` in, a shallower scale out — raw progress
                // for the geometry (a spring's overshoot is what makes it
                // spring), clamped for the alpha.
                let from = if self.panel.phase() == PresencePhase::Exiting {
                    PANEL_EXIT_SCALE
                } else {
                    PANEL_ENTER_SCALE
                };
                let scale = from + (1.0 - from) * progress;
                let lift = Vec2::new(0.0, (1.0 - progress) * PANEL_ENTER_LIFT);
                let centre = rect.center();
                let alpha = progress.clamp(0.0, 1.0) as f32;
                // A pushed layer clips to its own rectangle, so a layer sized to
                // the panel's *resting* `rect` cuts off whatever the transform
                // below moves past it — both the lift and, since the content
                // paints its own shadow outside its layout rect, that shadow's
                // own reach. `scale` never grows past `1.0` while `alpha` is
                // under it — both `PANEL_ENTER_SCALE` and `PANEL_EXIT_SCALE` are
                // under `1.0`, and `alpha < 1.0` is exactly the range this
                // branch scales *toward* `1.0` over — so the scale itself needs
                // no extra room; a fully open panel needs no layer at all,
                // since nothing is moved out of its own rect.
                //
                // `Rect::inflate` is symmetric and cannot express the shadow's
                // own directional reach (its `y_offset` widens only the
                // downward side), so the layer's bounds are built explicitly:
                // `SHADOW_SPILL_NEAR` on every side but the bottom, which also
                // carries `SHADOW_SPILL_FAR` and the lift's own travel (the
                // panel is lifted *down* from its resting position while
                // entering or exiting, so only that edge needs the extra
                // room).
                if alpha < 1.0 {
                    let layer = Rect::new(
                        rect.x0 - SHADOW_SPILL_NEAR,
                        rect.y0 - SHADOW_SPILL_NEAR,
                        rect.x1 + SHADOW_SPILL_NEAR,
                        rect.y1 + SHADOW_SPILL_FAR + lift.y.abs(),
                    );
                    scene.push_layer(layer.origin(), layer.size(), alpha);
                    layer_pushed = true;
                }
                scene.push_transform(
                    Affine::translate(lift)
                        * Affine::translate(centre.to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-centre.to_vec2()),
                );
            }
            ModalMount::Edge(edge) => {
                // The panel slides its own extent off the edge it is pinned to
                // and back — upstream's `x: "100%"` / `y: "100%"`, which never
                // fades the panel itself.
                let travel = if edge.is_vertical() {
                    rect.height()
                } else {
                    rect.width()
                };
                let away = edge.offscreen() * (travel * (1.0 - progress));
                scene.push_transform(Affine::translate(away));
            }
        }
        self.content.paint_child(ctx, scene);
        scene.pop_transform();
        if layer_pushed {
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A settled-closed host is inert: a broadcast still reaches the content
        // (which is what keeps its pods live), and nothing else does. It stays a
        // barrier for as long as it is visible, exit included — a press on a
        // panel still sliding out is the panel's, not the page's underneath.
        if !self.panel.is_visible() && !self.scrim.is_visible() {
            if event.is_broadcast() {
                self.content.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        self.drain_focus_release(ctx, event);
        // Claim focus on every `Down` — what makes Escape reachable, and what
        // keeps the root's focus session alive while the modal is up.
        // Re-claiming while focused is a no-op.
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            ctx.request_focus();
        }
        // Content first: an action inside the panel owns its own events, and
        // this host claims nothing before the routing — unless it already holds
        // the gesture, since a backdrop press the content declined must not be
        // re-routed into whatever control the pointer happens to travel over.
        // Broadcasts and focus-routed events are never short-circuited.
        let own_gesture = self.scrim_captured && matches!(event, InputEvent::Pointer(_));
        if !own_gesture && route_event_single(&mut self.content, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        if let InputEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Escape) {
                self.request_dismiss(ctx, event);
            }
            // Every other key is swallowed too: nothing behind a modal may act
            // on a keystroke its content declined.
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                // The barrier: swallow, and remember whether the press started
                // outside the panel. Button-agnostic, like every other barrier —
                // a modal blocks the page behind it whatever pressed it — and it
                // enters no press state of its own, so the primary-only press
                // rule has nothing to protect here.
                self.scrim_captured = true;
                self.scrim_down_outside = !self.rect.contains(p.position);
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.scrim_captured {
                    return EventResult::Ignored;
                }
                self.scrim_captured = false;
                // A backdrop *click*: pressed and released outside the panel,
                // matching the upstream backdrop being a button rather than a
                // pointerdown listener.
                if self.scrim_down_outside && !self.rect.contains(p.position) {
                    self.request_dismiss(ctx, event);
                }
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Swallowed while the barrier holds the gesture; otherwise
                // reported unhandled so nothing here claims hover it has no
                // chrome for.
                if self.scrim_captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Cancel => {
                // Flags only — a cancel never reaches app state.
                self.scrim_captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A panel on its way out is not there to be read, so the node exists
        // only while the modal is open.
        if !self.open || self.staged {
            return;
        }
        let label = self.label.clone();
        ctx.push_container(
            self.config.role.role(),
            |node| {
                node.set_modal();
                if let Some(label) = &label {
                    node.set_label(label.as_str());
                }
            },
            |ctx| self.content.semantics_child(ctx),
        );
    }

    visit_children!(content);
}

/// A modal component with the staged close and the back-press dismiss-signal
/// cell installed on the [`ModalWidget`] it builds.
///
/// [`show_modal`] is generic over every modal component, so it cannot reach the
/// [`ModalView`] each of them wraps privately; every one of them *does* build
/// the shared [`ModalWidget`] ([`ModalContent`]'s element bound), so this thin
/// pass-through installs both on the built widget instead. Its `rebuild` runs
/// after the inner one, so it is always the last writer.
struct StagedExit<V> {
    inner: V,
    on_close: OnClose,
    dismiss_signal: Option<Rc<Cell<u64>>>,
}

impl<State: 'static, V: ModalContent<State>> View<State> for StagedExit<V> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        let mut widget = self.inner.build(ctx);
        widget.set_on_close(Some(self.on_close.clone()));
        widget.set_dismiss_signal(self.dismiss_signal.clone());
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = self.inner.rebuild(&prev.inner, element, ctx);
        element.set_on_close(Some(self.on_close.clone()));
        // Deliberately not re-seeded: `set_dismiss_signal` would take the cell's
        // current generation as already-observed, which on the rebuild a back
        // press itself triggers would swallow that very bump.
        if element.dismiss_signal.is_none() {
            element.set_dismiss_signal(self.dismiss_signal.clone());
        }
        flags
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        self.inner.teardown(element, ctx);
    }
}

/// Push `build`'s modal as a transparent navigator page and register
/// `on_result` for the value it pops with — the shared push behind every modal
/// component's `show_*` wrapper.
///
/// The modal's dismissal is wired to `controller.pop()` as the staged
/// [`ModalView::on_close`] hook, so the panel animates out and pops only when
/// the exit settles; an action inside the content pops with a value of its own
/// through `controller.pop_with_result(..)`. The navigator transition is
/// [`TransitionSpec::NONE`] on purpose: the modal stages its own entrance *and*
/// exit, so a page transition on top of it would animate the same thing twice.
///
/// The back-press policy is peeked once, here, from
/// [`ModalContent::modal_dismissable`] — the push-time `PushOptions` contract
/// fixes it for the life of the page, even though `build` is re-invoked on every
/// later navigator rebuild to diff the page's content.
pub fn show_modal<State, V, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    V: ModalContent<State>,
    B: Fn() -> V + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    // Built **once**, outside the page builder, and only cloned into each build:
    // the builder re-runs on every navigator rebuild, and a fresh hook per pass
    // would throw away the depth an in-flight exit armed.
    let staged: OnClose = Rc::new(StagedPop::navigator_pop(controller));
    let dismissable = build().modal_dismissable();
    // A `Veto` page is pushed with no cell at all: the navigator consumes the
    // back press and fires nothing.
    let signal = dismissable.then(|| Rc::new(Cell::new(0u64)));
    let widget_signal = signal.clone();
    let mut options = PushOptions::transparent()
        .transition(TransitionSpec::NONE)
        .back(if dismissable {
            BackPolicy::DismissAnimated
        } else {
            BackPolicy::Veto
        })
        .on_result(on_result);
    if let Some(sig) = &signal {
        options = options.dismiss_signal(sig.clone());
    }
    controller.push_with_options(
        move || {
            any::<State, _>(StagedExit {
                inner: build(),
                on_close: staged.clone(),
                dismiss_signal: widget_signal.clone(),
            })
        },
        options,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{
        Color, KeyEvent, Modifiers, PointerButton, PointerEvent, text::TextContext,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(400.0, 600.0);
    const AREA: Size = Size::new(400.0, 600.0);

    // ---- geometry ---------------------------------------------------------

    /// The size the panel would be laid out at, for a config and an area.
    fn panel_size(config: ModalConfig, area: Size, content: Size) -> Size {
        panel_constraints(config, area).constrain(content)
    }

    #[test]
    fn a_centred_panel_takes_the_area_up_to_its_cap_inside_the_margin() {
        let config = ModalConfig::centered();
        // A desktop-width area caps at `max-w-sm`.
        let wide = panel_size(config, Size::new(1200.0, 800.0), Size::new(1000.0, 200.0));
        assert_eq!(wide.width, PANEL_MAX_WIDTH);
        assert_eq!(wide.height, 200.0, "the height hugs its content");
        // A phone-width one takes what is left inside the margins instead: the
        // cap never binds below `max-w-sm` plus the two margins.
        let narrow = panel_size(config, AREA, Size::new(1000.0, 200.0));
        assert_eq!(narrow.width, AREA.width - PANEL_MARGIN * 2.0);

        let rect = panel_rect(config, AREA, narrow);
        assert_eq!(rect.center().x, AREA.width / 2.0);
        assert_eq!(rect.center().y, AREA.height / 2.0);
    }

    #[test]
    fn a_hugged_height_is_capped_by_the_area_less_its_margins() {
        let config = ModalConfig::centered();
        let tall = panel_size(config, AREA, Size::new(200.0, 5_000.0));
        assert_eq!(tall.height, AREA.height - PANEL_MARGIN * 2.0);
    }

    #[test]
    fn a_side_drawer_is_its_own_width_full_height_and_flush_to_its_edge() {
        for (edge, flush) in [(ModalEdge::Left, 0.0), (ModalEdge::Right, 1.0)] {
            let config = ModalConfig::drawer(edge);
            let size = panel_size(config, AREA, Size::new(1000.0, 1000.0));
            assert_eq!(size.width, DRAWER_WIDTH);
            assert_eq!(size.height, AREA.height, "inset-y-0");
            let rect = panel_rect(config, AREA, size);
            if flush == 0.0 {
                assert_eq!(rect.x0, 0.0);
            } else {
                assert_eq!(rect.x1, AREA.width);
            }
            assert_eq!(rect.y0, 0.0);
        }
    }

    #[test]
    fn a_narrow_window_caps_the_drawer_at_its_viewport_fraction() {
        let config = ModalConfig::drawer(ModalEdge::Right);
        let narrow = Size::new(300.0, 600.0);
        let size = panel_size(config, narrow, Size::new(1000.0, 1000.0));
        assert_eq!(size.width, narrow.width * DRAWER_WIDTH_FRACTION);
    }

    #[test]
    fn a_bottom_sheet_is_the_full_width_at_its_snap_height_flush_to_the_bottom() {
        let config = ModalConfig::sheet();
        let size = panel_size(config, AREA, Size::new(10.0, 10.0));
        assert_eq!(size.width, AREA.width);
        assert_eq!(size.height, AREA.height * SHEET_HEIGHT_FRACTION);
        let rect = panel_rect(config, AREA, size);
        assert_eq!(rect.y1, AREA.height);
        assert_eq!(rect.x0, 0.0);
    }

    #[test]
    fn every_edge_slides_away_from_the_edge_it_is_pinned_to() {
        assert_eq!(ModalEdge::Left.offscreen(), Vec2::new(-1.0, 0.0));
        assert_eq!(ModalEdge::Right.offscreen(), Vec2::new(1.0, 0.0));
        assert_eq!(ModalEdge::Top.offscreen(), Vec2::new(0.0, -1.0));
        assert_eq!(ModalEdge::Bottom.offscreen(), Vec2::new(0.0, 1.0));
        assert!(ModalEdge::Bottom.is_vertical() && !ModalEdge::Left.is_vertical());
    }

    #[test]
    fn an_unbounded_height_collapses_the_area_and_the_panel_with_it() {
        // The scroll-view trap `super::finite_or_zero` documents: the barrier
        // still swallows presses while the panel has no height to be pressed in.
        let config = ModalConfig::centered();
        let collapsed = panel_constraints(config, Size::new(WINDOW.width, 0.0));
        assert_eq!(collapsed.max().height, 0.0);
    }

    // ---- the host ---------------------------------------------------------

    #[derive(Default)]
    struct AppState {
        dismissed: u32,
        closed: u32,
        pressed: u32,
        open: bool,
    }

    /// A fixed-size leaf reporting every press into the app state.
    struct Panel(Size);

    /// The retained half of [`Panel`].
    struct PanelWidget(Size);

    impl View<AppState> for Panel {
        type Element = PanelWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PanelWidget {
            PanelWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut PanelWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for PanelWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            if p.phase == PointerPhase::Down {
                ctx.state_mut::<AppState>().pressed += 1;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    /// A fixed-size leaf that paints its own drop shadow outside its layout
    /// rect, exactly the reach [`SHADOW_SPILL_NEAR`]/[`SHADOW_SPILL_FAR`] are
    /// derived to cover — the geometry a fade layer must stay wide enough not
    /// to clip.
    struct ShadowedPanel(Size);

    /// The retained half of [`ShadowedPanel`].
    struct ShadowedPanelWidget(Size);

    impl View<AppState> for ShadowedPanel {
        type Element = ShadowedPanelWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ShadowedPanelWidget {
            ShadowedPanelWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ShadowedPanelWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for ShadowedPanelWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            let origin = ctx.origin();
            let size = ctx.size();
            scene.fill_rect(origin, size, Color::BLACK);
            // The shadow's own visual reach: `SHADOW_SPILL_NEAR` on the sides
            // with no offset, `SHADOW_SPILL_FAR` on the downward one.
            let shadow_origin =
                Point::new(origin.x - SHADOW_SPILL_NEAR, origin.y - SHADOW_SPILL_NEAR);
            let shadow_size = Size::new(
                size.width + SHADOW_SPILL_NEAR * 2.0,
                size.height + SHADOW_SPILL_NEAR + SHADOW_SPILL_FAR,
            );
            scene.fill_rect(shadow_origin, shadow_size, Color::BLACK);
        }
    }

    const CONTENT: Size = Size::new(320.0, 200.0);
    /// A frame time well past every entrance ramp in the default config.
    const SETTLED: u64 = 4_000;
    /// A span longer than both the panel's exit and the scrim's fade.
    const EXIT_SPAN: u64 = 400;

    /// A recording scene: the filled rects in paint order (the scrim first, then
    /// the panel), the alpha of each composited layer, and each layer's own
    /// recorded rectangle (origin, size) alongside it.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size)>,
        alphas: Vec<f32>,
        layers: Vec<(Point, Size)>,
        scrim_alpha: Option<f32>,
    }

    impl PaintScene for Recorder {
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.alphas.push(alpha);
            self.layers.push((origin, size));
        }
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            if size == WINDOW && self.rects.is_empty() {
                self.scrim_alpha = Some(color.components[3]);
            }
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    /// The host mounted as the top child of a full-area [`frust::Stack`], with
    /// the app owning the open flag.
    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        config: ModalConfig,
        closed: Rc<Cell<u32>>,
        stage: bool,
        shadowed: bool,
    }

    impl Harness {
        fn new(config: ModalConfig) -> Self {
            Self::build(config, false, false, None)
        }

        /// A host whose dismissal stages its own exit against a state-free close
        /// — the navigator shape, without a navigator.
        fn staged(config: ModalConfig) -> Self {
            Self::build(config, true, false, None)
        }

        /// A staged host whose content paints its own shadow outside its
        /// layout rect (see [`ShadowedPanel`]) — for a layer-containment
        /// check that needs a real shadow to contain.
        fn shadowed_staged(config: ModalConfig) -> Self {
            Self::build(config, true, true, None)
        }

        fn build(config: ModalConfig, stage: bool, shadowed: bool, theme: Option<Theme>) -> Self {
            let mut root = RenderRoot::new();
            if let Some(theme) = theme {
                root.set_theme(Box::new(theme));
            }
            let mut h = Harness {
                root,
                state: AppState {
                    open: true,
                    ..AppState::default()
                },
                tcx: TextContext::new(),
                config,
                closed: Rc::new(Cell::new(0)),
                stage,
                shadowed,
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            let config = self.config;
            let closed = self.closed.clone();
            let stage = self.stage;
            let shadowed = self.shadowed;
            let mut logic = move |s: &mut AppState| {
                let closed = closed.clone();
                let on_dismiss = |s: &mut AppState| {
                    s.dismissed += 1;
                    s.open = false;
                };
                let view: AnyView<AppState> = if shadowed {
                    let view = modal(ShadowedPanel(CONTENT), config)
                        .open(s.open)
                        .label("Settings")
                        .on_dismiss(on_dismiss);
                    if stage {
                        any(view.on_close(move || closed.set(closed.get() + 1)))
                    } else {
                        any(view)
                    }
                } else {
                    let view = modal(Panel(CONTENT), config)
                        .open(s.open)
                        .label("Settings")
                        .on_dismiss(on_dismiss);
                    if stage {
                        any(view.on_close(move || closed.set(closed.get() + 1)))
                    } else {
                        any(view)
                    }
                };
                frust::Stack(vec![view])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            self.state.closed = self.closed.get();
        }

        fn paint(&mut self, ms: u64) -> Recorder {
            let mut rec = Recorder::default();
            self.root
                .paint(&mut rec, FrameTime::from_nanos(ms * 1_000_000));
            self.state.closed = self.closed.get();
            rec
        }

        /// Paint the modal to rest: a `Presence` times its ramp from the frame
        /// it first painted, so the first pass latches the entrance clock and
        /// the second lands well past it. Returns the settled recorder.
        fn settle(&mut self) -> Recorder {
            self.paint(0);
            self.paint(SETTLED)
        }

        /// Run a staged exit to completion, the same two passes: one to latch
        /// its clock, one past both ramps.
        fn run_exit(&mut self) {
            self.paint(SETTLED);
            self.paint(SETTLED + EXIT_SPAN);
        }

        fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventResult {
            let outcome = self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
            if outcome.handled {
                EventResult::Handled
            } else {
                EventResult::Ignored
            }
        }

        /// A full press-and-release at one point.
        fn click(&mut self, x: f64, y: f64) {
            self.pointer(PointerPhase::Down, x, y);
            self.pointer(PointerPhase::Up, x, y);
        }

        fn key(&mut self, key: Key) -> bool {
            self.root
                .event(
                    &mut self.state,
                    &InputEvent::Key(KeyEvent {
                        key,
                        modifiers: Modifiers::default(),
                        repeat: false,
                    }),
                )
                .handled
        }

        fn escape(&mut self) -> bool {
            self.key(Key::Named(NamedKey::Escape))
        }
    }

    /// Where the centred panel lands in the test window.
    fn centred_rect() -> Rect {
        let config = ModalConfig::centered();
        panel_rect(config, AREA, panel_size(config, AREA, CONTENT))
    }

    #[test]
    fn the_host_paints_the_scrim_over_the_whole_area_and_the_panel_at_its_mount() {
        let mut h = Harness::new(ModalConfig::centered());
        let rec = h.settle();
        assert_eq!(rec.rects[0], (Point::ORIGIN, WINDOW), "scrim first");
        assert_eq!(rec.rects[1].0, centred_rect().origin(), "then the panel");
        let alpha = rec.scrim_alpha.expect("a scrim was painted");
        assert!((alpha - super::super::SCRIM_ALPHA).abs() < 1e-6);
    }

    #[test]
    fn a_scrimless_config_paints_only_its_panel_and_still_swallows_presses() {
        let mut h = Harness::new(ModalConfig::centered().scrim(false));
        let rec = h.settle();
        assert_eq!(rec.rects.len(), 1, "the panel alone");
        assert_eq!(rec.rects[0].0, centred_rect().origin());
        assert_eq!(
            h.pointer(PointerPhase::Down, 2.0, 2.0),
            EventResult::Handled,
            "the barrier does not depend on the scrim being painted"
        );
    }

    #[test]
    fn the_scrim_fades_in_with_the_panel_and_asks_for_frames_until_it_settles() {
        let mut h = Harness::new(ModalConfig::centered());
        let first = h.paint(0);
        assert_eq!(first.scrim_alpha, Some(0.0), "the scrim starts clear");
        let mid = h.paint(SCRIM_FADE.as_millis() as u64 / 2);
        let half = mid.scrim_alpha.expect("still painting");
        assert!(half > 0.0 && half < super::super::SCRIM_ALPHA);
        let settled = h.paint(SETTLED);
        assert!(
            (settled.scrim_alpha.unwrap() - super::super::SCRIM_ALPHA).abs() < 1e-6,
            "settled at the role's own alpha"
        );
    }

    #[test]
    fn a_backdrop_click_dismisses_exactly_once_and_a_press_inside_the_panel_does_not() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        let c = centred_rect().center();
        h.click(c.x, c.y);
        assert_eq!(h.state.dismissed, 0, "a press on the panel is the panel's");
        assert_eq!(h.state.pressed, 1, "and reached its content");

        h.click(2.0, 2.0);
        assert_eq!(h.state.dismissed, 1);
        // The app's flag is false now; the next pass closes the host and a
        // second backdrop click dismisses nothing.
        h.pass();
        h.click(2.0, 2.0);
        assert_eq!(h.state.dismissed, 1, "dismissed exactly once");
    }

    #[test]
    fn a_press_that_starts_on_the_panel_and_ends_outside_it_never_dismisses() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        let c = centred_rect().center();
        h.pointer(PointerPhase::Down, c.x, c.y);
        h.pointer(PointerPhase::Up, 2.0, 2.0);
        assert_eq!(h.state.dismissed, 0, "a drag off the panel is not a click");
    }

    #[test]
    fn escape_dismisses_exactly_once_and_every_other_key_is_swallowed() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        // A press seats the focus the key routing needs.
        let c = centred_rect().center();
        h.click(c.x, c.y);
        assert!(h.key(Key::Named(NamedKey::Tab)), "the barrier eats it");
        assert_eq!(h.state.dismissed, 0);
        h.escape();
        assert_eq!(h.state.dismissed, 1);
        h.escape();
        assert_eq!(h.state.dismissed, 1, "dismissed exactly once");
    }

    #[test]
    fn a_backdrop_dismissal_releases_the_roots_focus_path_on_its_next_key_pass() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        // The `Down` half of the backdrop click is what seats the root's
        // focus session; the `Up` half is what tries — and, on the pointer
        // dispatch alone, fails — to hand it back.
        h.pointer(PointerPhase::Down, 2.0, 2.0);
        assert!(
            h.root.is_focus_active(),
            "the down seated the focus session"
        );
        h.pointer(PointerPhase::Up, 2.0, 2.0);
        assert_eq!(h.state.dismissed, 1);
        assert!(
            h.root.is_focus_active(),
            "the root's own event loop never consults a release on the \
             pointer dispatch, so it cannot have moved yet"
        );

        // A rebuild syncs `open` onto the widget, same as a real reactive app
        // — and the barrier, still mid-exit, still answers a key press.
        h.pass();
        assert!(h.escape(), "still up and still swallowing keys");
        assert!(
            !h.root.is_focus_active(),
            "the deferred release lands on this key pass"
        );
    }

    #[test]
    fn a_non_dismissable_modal_answers_neither_escape_nor_the_backdrop() {
        let mut h = Harness::new(ModalConfig::centered().dismissable(false));
        h.settle();
        let c = centred_rect().center();
        h.click(c.x, c.y);
        h.escape();
        h.click(2.0, 2.0);
        assert_eq!(h.state.dismissed, 0);
        assert!(h.state.open, "still up");
    }

    #[test]
    fn the_barrier_swallows_every_press_the_panel_declined() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        assert_eq!(
            h.pointer(PointerPhase::Down, 2.0, 2.0),
            EventResult::Handled
        );
        assert_eq!(
            h.pointer(PointerPhase::Move, 3.0, 3.0),
            EventResult::Handled,
            "moves during a captured barrier gesture stay captured"
        );
        assert_eq!(h.pointer(PointerPhase::Up, 3.0, 3.0), EventResult::Handled);
        // With no gesture in flight a bare move is reported unhandled — the host
        // claims no hover it has no chrome for.
        assert_eq!(
            h.pointer(PointerPhase::Move, 4.0, 4.0),
            EventResult::Ignored
        );
    }

    // ---- presence staging -------------------------------------------------

    #[test]
    fn a_closed_modal_stays_painted_for_its_whole_exit() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        h.state.open = false;
        h.pass();
        // The pass that latches the exit's own clock still paints everything.
        let mid = h.paint(SETTLED);
        assert_eq!(mid.rects.len(), 2, "scrim and panel both still there");

        // The panel's exit is shorter than the scrim's fade, so the scrim
        // outlives it…
        let after_panel = h.paint(SETTLED + PANEL_EXIT.as_millis() as u64 + 1);
        assert_eq!(after_panel.rects.len(), 1, "the scrim alone");
        // …and past both, nothing is painted at all.
        let after_all = h.paint(SETTLED + EXIT_SPAN);
        assert!(after_all.rects.is_empty());
        // A settled-closed host is input-transparent again.
        assert_eq!(
            h.pointer(PointerPhase::Down, 2.0, 2.0),
            EventResult::Ignored
        );
    }

    #[test]
    fn a_staged_dismissal_pops_only_after_the_exit_has_finished() {
        let mut h = Harness::staged(ModalConfig::centered());
        h.settle();
        h.click(2.0, 2.0);
        assert_eq!(h.state.dismissed, 1);
        assert_eq!(h.closed.get(), 0, "nothing popped yet");

        // Mid-exit: still painted, still not popped. The first of these two
        // passes is what latches the exit's clock.
        h.paint(SETTLED);
        let mid = h.paint(SETTLED + PANEL_EXIT.as_millis() as u64 / 2);
        assert_eq!(mid.rects.len(), 2);
        assert_eq!(h.closed.get(), 0);

        // Past both ramps the close fires, exactly once.
        h.paint(SETTLED + EXIT_SPAN);
        assert_eq!(h.closed.get(), 1);
        h.paint(SETTLED + EXIT_SPAN * 2);
        assert_eq!(h.closed.get(), 1, "the staged close fires once");
    }

    #[test]
    fn a_staged_modal_refuses_a_second_dismissal_while_its_exit_runs() {
        let mut h = Harness::staged(ModalConfig::centered());
        h.settle();
        h.click(2.0, 2.0);
        // A second backdrop click mid-exit changes nothing: the exit is already
        // staged and the close is owed exactly once.
        h.click(2.0, 2.0);
        h.escape();
        assert_eq!(h.state.dismissed, 1);
        h.run_exit();
        assert_eq!(h.closed.get(), 1);
    }

    #[test]
    fn a_back_press_signal_stages_the_same_exit_a_backdrop_click_does() {
        let signal = Rc::new(Cell::new(0u64));
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let view = modal::<AppState, _>(Panel(CONTENT), ModalConfig::centered())
            .on_close(move || hook.set(hook.get() + 1));
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        w.set_dismiss_signal(Some(signal.clone()));

        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));

        let paint = |w: &mut ModalWidget, ms: u64| {
            let mut ctx =
                PaintCtx::for_test(Point::ORIGIN, WINDOW, FrameTime::from_nanos(ms * 1_000_000));
            w.paint(&mut ctx, &mut Recorder::default());
        };
        paint(&mut w, 0);
        paint(&mut w, 4_000);
        assert_eq!(w.phase(), PresencePhase::Present);

        // The navigator's back press bumps the shared generation cell.
        signal.set(signal.get() + 1);
        paint(&mut w, 4_000);
        assert_eq!(w.phase(), PresencePhase::Exiting, "the exit is staged");
        assert_eq!(closed.get(), 0, "and nothing popped yet");
        paint(&mut w, 4_000 + SCRIM_FADE.as_millis() as u64 + 1);
        assert_eq!(closed.get(), 1);

        // A stale generation is never re-read as a fresh bump.
        paint(&mut w, 9_000);
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn a_non_dismissable_modal_ignores_a_back_press_signal() {
        let signal = Rc::new(Cell::new(0u64));
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let view = modal::<AppState, _>(Panel(CONTENT), ModalConfig::centered().dismissable(false))
            .on_close(move || hook.set(hook.get() + 1));
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        w.set_dismiss_signal(Some(signal.clone()));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, FrameTime::from_nanos(0));
        w.paint(&mut ctx, &mut Recorder::default());
        signal.set(signal.get() + 1);
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, WINDOW, FrameTime::from_nanos(4_000_000_000));
        w.paint(&mut ctx, &mut Recorder::default());
        assert_eq!(w.phase(), PresencePhase::Present, "unmoved");
        assert_eq!(closed.get(), 0);
    }

    #[test]
    fn an_edge_mounted_panel_slides_its_own_extent_rather_than_fading() {
        let mut h = Harness::new(ModalConfig::drawer(ModalEdge::Right));
        let first = h.paint(0);
        assert!(
            first.alphas.is_empty(),
            "an edge panel is composited at full opacity, never faded"
        );
        // At zero progress it is a full panel-width off the trailing edge; the
        // painted rect is the untransformed one, so this reads the scene's own
        // transform through the panel's recorded position instead.
        let settled = h.paint(SETTLED);
        let config = ModalConfig::drawer(ModalEdge::Right);
        let rect = panel_rect(
            config,
            AREA,
            panel_size(config, AREA, Size::new(999.0, 999.0)),
        );
        assert_eq!(settled.rects[1].0, rect.origin());
    }

    #[test]
    fn reduce_motion_collapses_both_ramps_and_still_stages_the_close() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut h = Harness::build(ModalConfig::centered(), true, false, Some(theme));
        // Present on the frame it first paints.
        let first = h.paint(0);
        assert!(
            first.alphas.is_empty(),
            "fully present needs no fade layer — nothing is moved out of its own rect"
        );
        assert!((first.scrim_alpha.unwrap() - super::super::SCRIM_ALPHA).abs() < 1e-6);
        // A dismissal still stages, still settles, still pops — once.
        h.click(2.0, 2.0);
        h.paint(1);
        assert_eq!(h.closed.get(), 1);
        h.paint(2);
        assert_eq!(h.closed.get(), 1);
    }

    #[test]
    fn the_composited_layer_always_contains_the_lifted_scaled_panel() {
        let mut h = Harness::new(ModalConfig::centered());
        let rect = centred_rect();
        let centre = rect.center();

        // The layer a given `alpha` needs, mirroring the paint's own geometry:
        // `lift` and `scale` both move monotonically with progress on
        // `SPRING_PANEL` (overdamped — no overshoot), so while `alpha < 1.0`
        // it equals the raw progress the transform used.
        let assert_contains = |alpha: f32, origin: Point, size: Size| {
            let layer = Rect::from_origin_size(origin, size);
            let alpha = alpha as f64;
            let lift_y = (1.0 - alpha) * PANEL_ENTER_LIFT;
            let scale = PANEL_ENTER_SCALE + (1.0 - PANEL_ENTER_SCALE) * alpha;
            let half_h = rect.height() * scale / 2.0;
            let bottom = centre.y + half_h + lift_y;
            let top = centre.y - half_h + lift_y;
            assert!(
                layer.y1 + 1e-6 >= bottom,
                "layer bottom {} must reach the lifted panel's bottom {bottom}",
                layer.y1
            );
            assert!(
                layer.y0 - 1e-6 <= top,
                "layer top {} must reach the lifted panel's top {top}",
                layer.y0
            );
        };

        // Progress 0 — the worked example: a 200px-tall panel
        // lifted 20px against only a few px of scale pull-in, so a layer left
        // at the resting rect clipped roughly the last 17px of it.
        let first = h.paint(0);
        assert_eq!(first.layers.len(), 1, "still fading, so still layered");
        let (origin, size) = first.layers[0];
        assert_contains(first.alphas[0], origin, size);

        // Mid-ramp too, not only at the extremes.
        let mid = h.paint(20);
        assert_eq!(mid.layers.len(), 1);
        assert!(
            mid.alphas[0] > 0.0 && mid.alphas[0] < 1.0,
            "actually mid-ramp"
        );
        let (origin, size) = mid.layers[0];
        assert_contains(mid.alphas[0], origin, size);

        // Settled: nothing left to fade, so no layer is pushed at all.
        let settled = h.paint(SETTLED);
        assert!(
            settled.layers.is_empty(),
            "a fully open panel needs no fade layer"
        );

        // An edge mount never fades the panel, so it never composites one
        // either, at progress 0 or mid-slide.
        let mut edge = Harness::new(ModalConfig::drawer(ModalEdge::Right));
        assert!(edge.paint(0).layers.is_empty());
        assert!(edge.paint(20).layers.is_empty());
    }

    #[test]
    fn the_layer_contains_a_content_painted_shadow_through_entry_and_exit() {
        let assert_shadow_contained = |rec: &Recorder| {
            assert_eq!(rec.layers.len(), 1, "still fading, so still layered");
            let layer = Rect::from_origin_size(rec.layers[0].0, rec.layers[0].1);
            // `rects`: `[0]` the scrim, `[1]` the panel's own black fill,
            // `[2]` the shadow it paints outside that fill.
            assert_eq!(rec.rects.len(), 3);
            let shadow = Rect::from_origin_size(rec.rects[2].0, rec.rects[2].1);
            assert!(
                layer.x0 - 1e-6 <= shadow.x0
                    && layer.y0 - 1e-6 <= shadow.y0
                    && layer.x1 + 1e-6 >= shadow.x1
                    && layer.y1 + 1e-6 >= shadow.y1,
                "layer {layer:?} clips the shadow {shadow:?}"
            );
        };

        // Entrance: progress 0 and mid-ramp.
        let mut h = Harness::shadowed_staged(ModalConfig::centered());
        assert_shadow_contained(&h.paint(0));
        assert_shadow_contained(&h.paint(20));

        // Exit: latch the exit's own clock at rest, then read mid-ramp — the
        // frame the exit starts on is still at full opacity (nothing to
        // contain yet, see the settled case above), so the first read worth
        // checking is partway through it, on `PANEL_EXIT_SCALE` rather than
        // `PANEL_ENTER_SCALE`.
        h.paint(SETTLED);
        h.click(2.0, 2.0);
        h.paint(SETTLED);
        assert_shadow_contained(&h.paint(SETTLED + PANEL_EXIT.as_millis() as u64 / 2));
    }

    #[test]
    fn a_reopened_modal_is_dismissable_again_and_publishes_its_semantics_node() {
        let mut h = Harness::staged(ModalConfig::centered());
        h.settle();
        h.click(2.0, 2.0);
        assert_eq!(
            h.state.dismissed, 1,
            "the backdrop click staged a dismissal"
        );
        assert!(!h.state.open, "on_dismiss flipped the app's own flag");
        // The flag change resyncs `open` onto the widget mid-exit, the way a
        // real reactive rebuild would.
        h.pass();
        h.run_exit();
        assert_eq!(h.closed.get(), 1, "the staged close settled and fired");

        // The app reopens the same kept-mounted host.
        h.state.open = true;
        h.pass();
        h.settle();

        let open = h.root.semantics();
        assert!(
            open.nodes.iter().any(|(_, n)| n.role() == Role::Dialog),
            "a reopened modal is announced again, not left latched from its prior exit"
        );
        // A press seats the focus the key routing needs.
        let c = centred_rect().center();
        h.click(c.x, c.y);
        h.escape();
        assert_eq!(
            h.state.dismissed, 2,
            "the reopened panel is dismissable again, not permanently latched"
        );
    }

    /// The same reopen guarantee, but flipped back on *while the exit is
    /// still animating* — `staged && !closed` — rather than after it has
    /// fully settled, which is the case the test above covers.
    #[test]
    fn reopening_before_the_exit_settles_is_dismissable_again_and_publishes_its_semantics_node() {
        let mut h = Harness::staged(ModalConfig::centered());
        h.settle();
        h.click(2.0, 2.0);
        assert_eq!(h.state.dismissed, 1);
        h.pass();
        // Latch the exit's clock, then read it partway through — still
        // running, and the staged close has not fired yet.
        h.paint(SETTLED);
        let mid = h.paint(SETTLED + PANEL_EXIT.as_millis() as u64 / 2);
        assert_eq!(mid.rects.len(), 2, "still mid-exit");
        assert_eq!(h.closed.get(), 0, "the staged close has not fired");

        h.state.open = true;
        h.pass();
        h.settle();

        let open = h.root.semantics();
        assert!(
            open.nodes.iter().any(|(_, n)| n.role() == Role::Dialog),
            "announced again, not left latched from the interrupted exit"
        );
        let c = centred_rect().center();
        h.click(c.x, c.y);
        h.escape();
        assert_eq!(
            h.state.dismissed, 2,
            "dismissable again after a reopen that interrupted the exit"
        );
    }

    // ---- the navigator seam -----------------------------------------------

    /// A state-free leaf, usable as a page background and as a modal panel.
    struct Block(Size);

    /// The retained half of [`Block`].
    struct BlockWidget(Size);

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<i32>>,
    }

    /// A navigator rooted on an opaque page, plus the controller driving it.
    struct NavHarness {
        controller: NavigatorController<NavState>,
        root: RenderRoot<NavState, frust::NavigatorView<NavState>>,
        state: NavState,
        tcx: TextContext,
    }

    impl NavHarness {
        fn new() -> Self {
            let mut h = NavHarness {
                controller: NavigatorController::new(),
                root: RenderRoot::new(),
                state: NavState::default(),
                tcx: TextContext::new(),
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            let ctrl = self.controller.clone();
            // The raw `frust-widgets` constructor rather than the facade's:
            // the facade wires platform back-press glue, which needs a
            // reactive runtime no unit test stands up.
            let mut app = move |_: &mut NavState| {
                frust_widgets::navigator(&ctrl, || any::<NavState, _>(Block(WINDOW)))
            };
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        /// One paint at `ms`, without a preceding rebuild — for driving a
        /// single frame of an already-staged ramp by hand. Returns whether
        /// another frame is wanted.
        fn paint(&mut self, ms: u64) -> bool {
            self.paint_rec(ms).0
        }

        /// [`Self::paint`], keeping the recorded scene instead of discarding
        /// it — for a check that needs to see what actually painted.
        fn paint_rec(&mut self, ms: u64) -> (bool, Recorder) {
            let mut rec = Recorder::default();
            let needs_frame = self
                .root
                .paint(&mut rec, FrameTime::from_nanos(ms * 1_000_000))
                .needs_frame;
            (needs_frame, rec)
        }

        /// Rebuild → layout → paint until nothing asks for another frame
        /// (bounded), the shape a real shell's loop takes.
        fn drive(&mut self, from_ms: u64) {
            for i in 0..24u64 {
                self.pass();
                if !self.paint(from_ms + i * 100) {
                    return;
                }
            }
            panic!("the frame loop never settled");
        }

        fn click(&mut self, x: f64, y: f64) {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                self.root.event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: PointerButton::Primary,
                    }),
                );
            }
        }
    }

    #[test]
    fn show_modal_pushes_a_transparent_page_that_pops_once_its_exit_settles() {
        let mut h = NavHarness::new();
        show_modal(
            &h.controller,
            || modal(Block(Size::new(200.0, 120.0)), ModalConfig::centered()),
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<i32>()),
        );
        h.drive(0);
        assert_eq!(h.controller.depth(), 2, "the modal is up");

        // A backdrop click stages the exit; the pop lands only once it settles.
        h.click(5.0, 5.0);
        assert_eq!(
            h.controller.depth(),
            2,
            "nothing pops during the exit ramp itself"
        );
        h.drive(3_000);
        assert_eq!(h.controller.depth(), 1);
        assert_eq!(h.state.results, vec![None], "popped with an empty result");
    }

    #[test]
    fn a_non_dismissable_modal_pushed_this_way_never_leaves_on_its_own() {
        let mut h = NavHarness::new();
        show_modal(
            &h.controller,
            || {
                modal(
                    Block(Size::new(200.0, 120.0)),
                    ModalConfig::centered().dismissable(false),
                )
            },
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<i32>()),
        );
        h.drive(0);
        assert_eq!(h.controller.depth(), 2);
        h.click(5.0, 5.0);
        h.drive(3_000);
        assert_eq!(h.controller.depth(), 2, "the barrier ate the click");
        assert!(h.state.results.is_empty());
    }

    /// The staged pop waits out a whole exit ramp, and a navigator pop always
    /// takes the *top* page: a page the app pushes inside that window must not
    /// be popped in the modal's place, and the modal must stay dismissable
    /// afterwards rather than latching itself closed over a page it never
    /// removed.
    #[test]
    fn a_page_pushed_during_the_exit_ramp_is_not_popped_in_the_modals_place() {
        let mut h = NavHarness::new();
        show_modal(
            &h.controller,
            || modal(Block(Size::new(200.0, 120.0)), ModalConfig::centered()),
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<i32>()),
        );
        h.drive(0);
        assert_eq!(h.controller.depth(), 2, "the modal is up");

        // A backdrop tap stages the exit…
        h.click(5.0, 5.0);

        // …and, one frame into the ramp, the app pushes a page of its own (a
        // deep link, an async completion landing). Transparent, so the modal
        // keeps painting underneath it and its ramp really does reach the
        // settle that fires the staged pop; an opaque page would cull the
        // modal's paint and the pop would never fire at all.
        h.pass();
        h.paint(3_000);
        h.controller
            .push_transparent(|| any::<NavState, _>(Block(Size::new(400.0, 100.0))));
        h.drive(3_100);

        assert_eq!(
            h.controller.depth(),
            3,
            "the pushed page survives — the staged pop was refused, not aimed at it"
        );
        assert!(
            h.state.results.is_empty(),
            "and nothing reported a dismissal"
        );

        // Recoverable, not bricked: back out of the pushed page, then dismiss
        // the modal again — the fresh exit is staged against the stack it
        // actually pops.
        h.controller.request_back();
        h.drive(6_000);
        assert_eq!(
            h.controller.depth(),
            2,
            "the pushed page backs out normally"
        );
        h.controller.request_back();
        h.drive(9_000);
        assert_eq!(
            h.state.results,
            vec![None],
            "the modal is still dismissible after the refused pop"
        );
        assert_eq!(h.controller.depth(), 1);
    }

    /// A refused pop must not resurrect a barrier the app's own `open` flag
    /// already says is gone — the state-bearing counterpart of the refusal
    /// above, where the app never flips its own flag and the panel *should*
    /// come back.
    #[test]
    fn a_refused_pop_does_not_resurrect_a_barrier_the_app_already_closed() {
        let mut h = NavHarness::new();
        let open = Rc::new(Cell::new(true));
        let open_for_build = open.clone();
        show_modal(
            &h.controller,
            move || {
                modal(Block(Size::new(200.0, 120.0)), ModalConfig::centered())
                    .open(open_for_build.get())
            },
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<i32>()),
        );
        h.drive(0);
        assert_eq!(h.controller.depth(), 2, "the modal is up");

        // Stage the exit, then let the app's own flag say it is already gone
        // — independent of whether the navigator's pop ever lands.
        h.click(5.0, 5.0);
        open.set(false);
        h.pass();
        h.paint(3_000);

        // The same push-during-the-ramp trick that forces the staged pop to
        // be refused (see the test above).
        h.controller
            .push_transparent(|| any::<NavState, _>(Block(Size::new(400.0, 100.0))));
        h.drive(3_100);
        assert_eq!(h.controller.depth(), 3, "the pushed page survives");
        assert!(h.state.results.is_empty(), "nothing reported a dismissal");

        // Every layer paints (the pushed page is transparent), so the only
        // rects left are the root page's and the pushed page's own — the
        // modal must not repaint a scrim or a panel once `open` says it is
        // gone, refusal or not.
        let (_, rec) = h.paint_rec(9_000);
        assert_eq!(
            rec.rects.len(),
            2,
            "the modal resurrected a barrier its own `open` flag already \
             closed"
        );
    }

    #[test]
    fn a_dismissed_modal_reports_no_semantics_node() {
        let mut h = Harness::new(ModalConfig::centered());
        h.settle();
        let open = h.root.semantics();
        assert!(
            open.nodes.iter().any(|(_, n)| n.role() == Role::Dialog),
            "an open modal is announced as a dialog"
        );
        h.state.open = false;
        h.pass();
        let closed = h.root.semantics();
        assert!(
            !closed.nodes.iter().any(|(_, n)| n.role() == Role::Dialog),
            "a panel on its way out is not there to be read"
        );
    }
}
