// Ported from `frust-shadcn`'s `plugins/shadcn/src/overlay/modal.rs` (in-repo
// sibling catalog; itself a port of shadcn/ui v4 rev
// `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, MIT © shadcn, over Radix's dialog
// and vaul's drawer). A sibling port, never a dependency — see `super`'s header.
// Porting decisions: the panel geometry, the staged-exit machine and the
// drag/snap-point gesture are carried over verbatim; the chrome resolves
// Material roles (32% scrim, `surfaceContainer*` fill, `shape.extra_large`
// corners, the M3 32×4dp drag handle, the M3 `close` icon) and the motion
// resolves `crate::tokens` presets. The one structural change is the back-press
// tier: this host sits on `crate::sheet`'s `BackPolicy::DismissAnimated` +
// `PushOptions::dismiss_signal` seam, and — unlike that sheet, which pops
// immediately — routes a back press into the *same* staged exit as every other
// dismiss gesture (`crate::dialog`'s simpler `push_transparent_for_result` path
// has no back seam at all, which is why it is not the one reused here).

//! The modal overlay host: the M3 32% scrim plus one panel, as a single widget
//! the modal families (dialog, alert dialog, bottom sheet, side sheet,
//! full-screen search, picker) configure rather than re-derive.
//!
//! Upstream these are wrappers over the *same* two elements — a full-area
//! overlay and a positioned content panel — differing only in where the content
//! is pinned, which corners it rounds, which edge it borders, and how it
//! animates in. [`OverlayModalConfig`] is exactly that difference list, so a
//! component module contributes its chrome config plus the composed content view
//! and nothing else.
//!
//! # Architecture
//!
//! Mirrors [`mod@crate::dialog`]/[`crate::sheet`] end to end (see [`super`]'s
//! mounting note): the scrim belongs to *this* widget rather than to the
//! navigator, the panel is a modal barrier that swallows every input its content
//! did not take, dismissal is the app's `on_dismiss` (wired to
//! `controller.pop()` by [`show_overlay_modal`]), and an action inside the
//! content pops with a value through the navigator's own pop-result machinery.
//!
//! Three pointer dismiss gestures, each independently disableable:
//!
//! * a **scrim tap** — a press *and* release outside the panel, when
//!   [`OverlayModalConfig::scrim_dismiss`] is on (an alert dialog turns it off:
//!   it demands an explicit choice);
//! * the **close button** — the top-trailing M3 `close` icon, when
//!   [`OverlayModalConfig::close_button`] is on;
//! * the **drag handle** — the M3 32×4dp bar, when
//!   [`OverlayModalConfig::handle`] is on. A press and release on it dismisses;
//!   with [`OverlayModalConfig::drag`] on it is also the drag affordance
//!   (below).
//!
//! plus **Escape**, once the modal holds focus — claimed on every `Down`, the
//! [`mod@crate::dialog`]/[`crate::sheet`] opt-in, with the same documented gap:
//! there is no auto-focus-on-appear hook in the framework, so a caller must
//! complete one pointer interaction with the modal before Escape does anything.
//!
//! [`OverlayModalView::dismissable`] (default `true`) is the one flag over all
//! of them: `false` disables the scrim tap, the close button, the handle, the
//! drag commit, Escape *and* the back press together, leaving only an explicit
//! control the app wires inside its own content.
//!
//! # The barrier swallows keys too
//!
//! The panel is a modal barrier for *every* input class, not just pointers: a
//! key the content did not take is reported [`EventResult::Handled`] rather than
//! falling through to whatever sits behind the modal. Content-first routing is
//! unchanged — a focused field inside the panel sees every keystroke, and only
//! what it declines reaches the barrier, where Escape dismisses and everything
//! else is absorbed.
//!
//! # Motion
//!
//! [`OverlayEntrance::FadeScale`] composites the panel through a
//! [`PaintScene::push_layer`] at the ramp's alpha, under a scale about its own
//! centre. [`OverlayEntrance::Slide`] moves the panel's origin in **layout** so
//! hit-testing follows the panel rather than lagging behind a paint-only
//! transform — which is also why a running slide asks for `request_layout`, not
//! a bare `request_frame`. The scrim fades with either.
//!
//! The source's Tailwind timings map onto `crate::tokens`' M3 presets:
//!
//! | Source class | Material preset |
//! |---|---|
//! | `duration-200` (dialog content) | [`MaterialMotion::SHORT_4`] (200ms) |
//! | `duration-500` (sheet content) | [`MaterialMotion::LONG_2`] (500ms) |
//! | `ease-out` (fade/zoom in) | [`MaterialMotion::EMPHASIZED_DECELERATE`] |
//! | `ease-in-out` (slide in) | [`MaterialMotion::EMPHASIZED`] |
//! | (either, on the way out) | [`MaterialMotion::EMPHASIZED_ACCELERATE`] — M3's curve for elements leaving |
//! | `zoom-in-95` | [`SCALE_FROM`], carried over verbatim (a visual constant, not a token) |
//!
//! An exit runs the same ramp backwards over the same duration scaled by how
//! much travel is left, so a half-open drawer closes in half the time.
//!
//! `Theme.motion.reduce_motion` collapses both to a jump: the ramp is stopped
//! and progress snaps to its rest value on the first paint, one frame after the
//! modal mounts (the pass that sets it is a paint, and the geometry it feeds is
//! a layout) — so a reduced-motion modal appears whole on its second frame
//! rather than animating on its first.
//!
//! # Exit motion, and why the close hook is state-free
//!
//! Every dismiss trigger — scrim tap, Escape, the close button, the handle, a
//! drag past its threshold, **an Android back press** — stages an **exit**
//! instead of firing the app's dismissal on the spot: the same `progress` ramp
//! runs back down to `0.0`, the barrier keeps swallowing input the whole way, a
//! second trigger mid-exit is a no-op, and only when the ramp settles is the
//! dismissal fired — from `paint`, which is sound because
//! [`NavigatorController::pop`] merely *enqueues* an op applied on the next
//! rebuild (the [`crate::sheet`]/`frust_glyph::dialog` precedent; the same paint
//! asks for one more frame so that rebuild is guaranteed to come).
//!
//! Firing from paint is why the staged path's callback is
//! [`OverlayModalView::on_close`] — a plain `Fn()` — and not the
//! `Fn(&mut State)` [`OverlayModalView::on_dismiss`] takes: `PaintCtx` carries
//! no app state. [`show_overlay_modal`] wires `on_close` to `controller.pop()`
//! for every component, so the navigator path animates out with no app
//! involvement, and app state rides the navigator's own `on_result` (delivered
//! with `&mut State` after the pop) exactly as before. **A modal with no
//! `on_close` wired** — a `Stack` mount that only set `on_dismiss` — keeps the
//! immediate, unstaged dismissal: there is no state-bearing pass to defer into,
//! so staging one would leave an invisible barrier standing. `reduce_motion`
//! collapses the exit the same way it collapses the entrance.
//!
//! # Back-dismiss: the `DismissAnimated` tier
//!
//! [`show_overlay_modal`] pushes its transparent page with
//! [`frust::BackPolicy::DismissAnimated`] when the modal is
//! [`dismissable`](OverlayModalView::dismissable) and
//! [`frust::BackPolicy::Veto`] when it is not — the same tier
//! [`crate::sheet`] uses, and the reason this host does *not* reuse
//! [`mod@crate::dialog`]'s plain `push_transparent_for_result` path (which has no
//! back seam). A back press therefore never pops the page itself: the navigator
//! bumps the shared [`PushOptions::dismiss_signal`] generation cell, this
//! widget observes the bump on its next paint
//! ([`observe_dismiss_signal`](OverlayModalWidget::observe_dismiss_signal)) and
//! stages the ordinary exit ramp, which pops on settle. A `Veto` page is pushed
//! with no cell at all, so a back press is consumed and does nothing.
//!
//! **The drain gotcha** (documented on [`crate::sheet`]'s own observer, and the
//! reason both fire paths request a frame): `pop()` only *enqueues* a nav op and
//! writes no tracked signal, so nothing else schedules the frame whose rebuild
//! drains it. Without a [`PaintCtx::request_frame`] from the very paint that
//! fires, a dirty-driven desktop shell (`ControlFlow::Wait`) idles and the
//! mobile frame gate skips, leaving the dismissal dead until an unrelated later
//! frame.
//!
//! # Drag-to-close and snap points
//!
//! [`OverlayModalConfig::drag`] turns an edge-pinned panel into a draggable
//! sheet: a press on the handle — or anywhere on the panel the content did not
//! take — captures, each move scrubs `progress` along the panel's own axis so
//! the panel tracks the pointer (the scrim dims with it), and the release picks
//! the nearest resting point, with a fast flick ([`OVERLAY_FLING_VELOCITY`])
//! nudging to the next one in the direction of travel. Landing on `0.0`
//! continues into the exit ramp above rather than snapping shut. A press that
//! never passes [`TOUCH_SLOP`] is a *tap*: on the handle it dismisses, anywhere
//! else it is swallowed like any other barrier press.
//!
//! [`OverlayModalView::snap_points`]: a value in `0..=1` is a fraction of the
//! panel's own extent, a value above `1` is logical px. The panel opens to the
//! **first** point (fully open with none given), and the resting set the release
//! snaps to is those points plus `0.0` (closed) — so with no snap points at all
//! the two candidates are `0.0` and `1.0` and "nearest" is exactly the
//! conventional drag-past-the-midpoint commit ([`crate::sheet`]'s own
//! `DRAG_DISMISS_FRACTION` of `0.5`, arrived at from the other direction). A
//! first point that itself normalizes to `0.0` is the one exception:
//! [`OverlayModalWidget::open_progress`] floors it to fully open rather than
//! authoring a panel with nothing to open to. The panel is laid out at its full
//! extent and translated to expose the active fraction, so a partially-open
//! sheet carries a proportionally lighter scrim rather than a full-strength one.
//!
//! # Dragging a non-dismissable panel
//!
//! [`OverlayModalConfig::drag`] stays on whether or not the panel is
//! dismissable — the *gesture* is never gated on whether a close callback
//! exists, only the *outcome* is. A drag past the closing threshold on a panel
//! that cannot dismiss springs back to
//! [`open_progress`](OverlayModalWidget::open_progress) instead of continuing
//! into the exit ramp ([`settle_drag`](OverlayModalWidget::settle_drag)) — the
//! same reachability rule the Escape handler applies.
//!
//! # Clamp discipline
//!
//! `f64::clamp` panics if `min > max` or either bound is NaN — a hard abort
//! under `panic = "abort"`, not a catchable error (see [`crate::slider`]'s own
//! core for the same rule stated at length, which this one mirrors).
//! [`OverlayModalView::snap_points`] is the one place caller input enters
//! this module, and it drops any non-finite point right there — the rest of
//! the module (`normalize_snap`, `snap_targets`, `open_progress`) trusts
//! [`OverlayModalWidget::snap_points`] to already be all-finite rather than
//! re-checking. That leaves two clamp shapes in the module tree:
//!
//! - **Bounds are literal `0.0`/`1.0`** (`normalize_snap`'s two clamps,
//!   [`begin_ramp`](OverlayModalWidget::begin_ramp)'s fraction clamp) —
//!   trivially ordered, never a hazard.
//! - **A computed bound clamped with the ordering-visible `.max()`/`.min()`
//!   shape instead of `.clamp()`** —
//!   [`drag_move`](OverlayModalWidget::drag_move)'s scrub target against the
//!   live `open` snap-target bound: `f64::max`/`f64::min` can't panic even if
//!   a NaN somehow reached one side (IEEE `minNum`/`maxNum` semantics return
//!   the other, non-NaN operand), which `clamp`'s internal
//!   `assert!(min <= max)` would.
//!
//! A new clamp against a computed bound must fit one of these two shapes, not
//! introduce a third.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CornerRadii, CursorIcon, ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx,
    NamedKey, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View,
    Widget, any, build_child, erase_callback, rebuild_child, route_event_single, teardown_child,
    visit_children,
};
use frust::input::{TOUCH_SLOP, VelocityTracker};
use frust::{
    AnimationController, BackPolicy, Curve, FrameTime, IconData, NavigatorController, PopResult,
    PushOptions, Theme, TransitionSpec,
};

use super::{OverlayContainer, OverlayElevation, OverlaySide, finite_or_zero, reduce_motion};
use crate::press::presses;
use crate::tokens::{MaterialMotion, MaterialSpacing};

/// Minimum dialog panel width, in logical px (M3 dialog spec; the same bound
/// [`mod@crate::dialog`] clamps to).
pub const OVERLAY_DIALOG_MIN_WIDTH: f64 = 280.0;
/// Maximum dialog panel width, in logical px (M3 dialog spec).
pub const OVERLAY_DIALOG_MAX_WIDTH: f64 = 560.0;
/// The margin a centred panel keeps on *each* side of the viewport —
/// [`MaterialSpacing::LG`] (16dp), where the source's own
/// `max-w-[calc(100%-2rem)]` lands.
const CENTERED_MARGIN: f64 = MaterialSpacing::LG;
/// A side sheet's share of the viewport width (the source's `w-3/4`).
pub const OVERLAY_EDGE_FRACTION: f64 = 0.75;
/// Maximum side-sheet width, in logical px (M3 side sheets span 256–400dp).
pub const OVERLAY_SIDE_SHEET_MAX_WIDTH: f64 = 400.0;
/// A bottom sheet's share of the viewport height, for a config that caps it.
pub const OVERLAY_SHEET_MAX_HEIGHT_FRACTION: f64 = 0.8;

/// The scale a [`OverlayEntrance::FadeScale`] entrance starts from (the
/// source's `zoom-in-95`, carried over verbatim — a visual constant with no
/// Material token to resolve against; the same value
/// [`super::ANCHORED_ENTER_SCALE`] carries for the anchored host).
pub const SCALE_FROM: f64 = 0.95;
/// Progress difference below which a ramp counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

/// Release speed, in logical px/s along the drag axis, at or above which a drag
/// commits in the direction it was flung rather than to whichever resting point
/// is nearest.
///
/// **Community-approximate**: Material publishes no fling threshold for a sheet
/// dismiss. 400 px/s (0.4 px/ms) is a deliberate flick — an order of magnitude
/// above `frust::input::FLING_STOP`'s 30 px/s "this fling is over" floor, and
/// well under the speed of a full-screen swipe.
pub const OVERLAY_FLING_VELOCITY: f64 = 400.0;

/// The close affordance's inset from the panel's top/trailing edges —
/// [`MaterialSpacing::LG`] (16dp).
const CLOSE_INSET: f64 = MaterialSpacing::LG;
/// The close icon's painted size, in logical px (the M3 24dp icon box).
const CLOSE_ICON: f64 = 24.0;
/// The close affordance's **hit** box, in logical px: the M3 48dp minimum touch
/// target centred on the 24dp icon. Chrome-only — it changes no layout and no
/// painted pixel.
const CLOSE_HIT: f64 = 48.0;

/// Drag-handle visual indicator width, in logical px (M3 token
/// `m3_comp_sheet_bottom_docked_drag_handle_width`; [`crate::sheet`]'s own).
const HANDLE_WIDTH: f64 = 32.0;
/// Drag-handle visual indicator height, in logical px (M3 token
/// `m3_comp_sheet_bottom_docked_drag_handle_height`).
const HANDLE_HEIGHT: f64 = 4.0;
/// The M3 48dp minimum touch target the handle is hit-tested and laid out
/// against ([`crate::sheet`]'s handle strip).
const HANDLE_TOUCH_TARGET: f64 = 48.0;

/// The vertical space a drag handle occupies at the top of a panel — the inset
/// a sheet's own content reserves for it, since the handle is painted as panel
/// chrome rather than laid out as a child.
pub const OVERLAY_HANDLE_RESERVE: f64 = HANDLE_TOUCH_TARGET;

/// How far outside the panel its elevation shadow may reach, in logical px —
/// the bound the fade layer is inflated by so the shadow is not clipped out of
/// it.
const SHADOW_SPILL: f64 = 48.0;

/// How much of an axis a panel takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayExtent {
    /// A fraction of the area's own extent on that axis.
    Fraction(f64),
    /// Whatever the content asks for.
    Content,
}

/// An upper bound on a resolved [`OverlayExtent`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayLimit {
    /// No cap beyond the area itself.
    None,
    /// A fixed cap in logical px.
    Px(f64),
    /// A cap as a fraction of the area's extent.
    Fraction(f64),
}

impl OverlayLimit {
    /// Apply this cap to `value`, given the area's extent on the same axis.
    fn apply(self, value: f64, area: f64) -> f64 {
        let cap = match self {
            OverlayLimit::None => area,
            OverlayLimit::Px(px) => px.min(area),
            OverlayLimit::Fraction(f) => area * f,
        };
        value.min(cap)
    }
}

/// Where the panel sits inside the host area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayGeometry {
    /// Centred both ways, content-tall, capped at `max_width` (the M3 dialog
    /// shape).
    Centered {
        /// The width cap, in logical px.
        max_width: f64,
    },
    /// Pinned to one edge, full-bleed on the cross axis (the M3 bottom-sheet
    /// and side-sheet shapes).
    Edge {
        /// The edge the panel is pinned to (and slides in from).
        side: OverlaySide,
        /// The panel's extent *along* that edge's axis (its thickness).
        extent: OverlayExtent,
        /// The cap on that extent.
        limit: OverlayLimit,
    },
}

/// Which of the panel's corners are rounded (all at the theme's
/// `shape.extra_large`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayCorners {
    /// All four — the M3 dialog.
    #[default]
    All,
    /// Square — a full-height side sheet meeting three screen edges.
    None,
    /// The top two — an M3 bottom sheet.
    Top,
    /// The bottom two — a top sheet.
    Bottom,
    /// The leading two — a trailing-edge side sheet.
    Start,
    /// The trailing two — a leading-edge side sheet.
    End,
}

impl OverlayCorners {
    /// The per-corner radii for a `radius`-sized corner.
    fn radii(self, radius: f64) -> CornerRadii {
        let (tl, tr, br, bl) = match self {
            OverlayCorners::All => (radius, radius, radius, radius),
            OverlayCorners::None => (0.0, 0.0, 0.0, 0.0),
            OverlayCorners::Top => (radius, radius, 0.0, 0.0),
            OverlayCorners::Bottom => (0.0, 0.0, radius, radius),
            OverlayCorners::Start => (radius, 0.0, 0.0, radius),
            OverlayCorners::End => (0.0, radius, radius, 0.0),
        };
        CornerRadii::new(tl, tr, br, bl)
    }
}

/// Which of the panel's edges carry a 1dp outline.
///
/// M3 gives neither a dialog nor a sheet a border by default (elevation and the
/// container role separate them from the scrim), so both presets below leave
/// this [`OverlayBorder::None`]; the arms exist for a component that needs a
/// divider against an opaque background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayBorder {
    /// No border at all.
    #[default]
    None,
    /// All four edges.
    All,
    /// One edge only — a side sheet's *inner* edge.
    Edge(OverlaySide),
}

/// The 1dp hairline weight an [`OverlayBorder`] strokes at.
const BORDER_WIDTH: f64 = 1.0;

/// How the panel enters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayEntrance {
    /// Fade plus a scale about the panel's centre — paint-only.
    #[default]
    FadeScale,
    /// Slide in off the geometry's own edge — driven in layout.
    Slide,
    /// No entrance: the panel is at rest on its first frame.
    None,
}

impl OverlayEntrance {
    /// This entrance's ramp duration (see the module docs' motion table).
    fn duration(self) -> Duration {
        match self {
            OverlayEntrance::FadeScale => MaterialMotion::SHORT_4,
            OverlayEntrance::Slide => MaterialMotion::LONG_2,
            OverlayEntrance::None => Duration::ZERO,
        }
    }

    /// This entrance's easing, per direction: M3's decelerate/standard curves
    /// entering, its accelerate curve leaving.
    fn curve(self, exiting: bool) -> Curve {
        if exiting {
            return MaterialMotion::EMPHASIZED_ACCELERATE;
        }
        match self {
            OverlayEntrance::Slide => MaterialMotion::EMPHASIZED,
            _ => MaterialMotion::EMPHASIZED_DECELERATE,
        }
    }

    /// Whether the entrance moves the panel's *geometry* (and therefore needs a
    /// relayout per frame rather than a repaint).
    fn is_layout_affecting(self) -> bool {
        matches!(self, OverlayEntrance::Slide)
    }
}

/// The accessibility role the modal reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayRole {
    /// A dialog box (`Role::Dialog`) — also the role
    /// [`mod@crate::dialog`]/[`crate::sheet`] report, accesskit having no
    /// dedicated sheet role.
    #[default]
    Dialog,
    /// An alert dialog demanding a response (`Role::AlertDialog`).
    AlertDialog,
}

impl OverlayRole {
    /// The accesskit role.
    fn role(self) -> Role {
        match self {
            OverlayRole::Dialog => Role::Dialog,
            OverlayRole::AlertDialog => Role::AlertDialog,
        }
    }
}

/// The chrome differences between the modal families — see the [module
/// docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayModalConfig {
    /// Where the panel sits.
    pub geometry: OverlayGeometry,
    /// Which corners round.
    pub corners: OverlayCorners,
    /// Which edges carry an outline.
    pub border: OverlayBorder,
    /// The `surfaceContainer*` rung the panel fills with.
    pub container: OverlayContainer,
    /// The panel's elevation rung.
    pub elevation: OverlayElevation,
    /// How the panel enters.
    pub entrance: OverlayEntrance,
    /// Whether a press+release outside the panel dismisses.
    pub scrim_dismiss: bool,
    /// Whether the close affordance is shown.
    pub close_button: bool,
    /// Whether the drag-handle bar is shown (and dismisses on release).
    pub handle: bool,
    /// Whether the panel can be dragged along its own edge axis to close (see
    /// the [module docs](self)). Meaningful only for an
    /// [`OverlayGeometry::Edge`] panel.
    pub drag: bool,
    /// The accessibility role.
    pub role: OverlayRole,
}

impl OverlayModalConfig {
    /// The M3 dialog chrome: a centred, fully-rounded `surfaceContainerHigh`
    /// panel at elevation 3, capped at `max_width`, fading and scaling in.
    pub fn centered(max_width: f64) -> Self {
        OverlayModalConfig {
            geometry: OverlayGeometry::Centered { max_width },
            corners: OverlayCorners::All,
            border: OverlayBorder::None,
            container: OverlayContainer::High,
            elevation: OverlayElevation::Level3,
            entrance: OverlayEntrance::FadeScale,
            scrim_dismiss: true,
            close_button: false,
            handle: false,
            drag: false,
            role: OverlayRole::Dialog,
        }
    }

    /// The M3 sheet chrome on `side`: a `surfaceContainerLow` panel at
    /// elevation 1 pinned to that edge, rounded on the two corners facing into
    /// the screen, sliding in.
    ///
    /// A left/right side sheet is [`OVERLAY_EDGE_FRACTION`] of the width capped
    /// at [`OVERLAY_SIDE_SHEET_MAX_WIDTH`] and full height; a top/bottom sheet
    /// is content-tall and full width.
    pub fn edge(side: OverlaySide) -> Self {
        let (extent, limit) = if side.is_vertical() {
            (OverlayExtent::Content, OverlayLimit::None)
        } else {
            (
                OverlayExtent::Fraction(OVERLAY_EDGE_FRACTION),
                OverlayLimit::Px(OVERLAY_SIDE_SHEET_MAX_WIDTH),
            )
        };
        let corners = match side {
            OverlaySide::Bottom => OverlayCorners::Top,
            OverlaySide::Top => OverlayCorners::Bottom,
            OverlaySide::Left => OverlayCorners::End,
            OverlaySide::Right => OverlayCorners::Start,
        };
        OverlayModalConfig {
            geometry: OverlayGeometry::Edge {
                side,
                extent,
                limit,
            },
            corners,
            border: OverlayBorder::None,
            container: OverlayContainer::Low,
            elevation: OverlayElevation::Level1,
            entrance: OverlayEntrance::Slide,
            scrim_dismiss: true,
            close_button: false,
            handle: false,
            drag: false,
            role: OverlayRole::Dialog,
        }
    }

    /// Replace the panel's extent along its own axis (an
    /// [`OverlayGeometry::Edge`] config only; a no-op on a centred one).
    pub fn extent(mut self, extent: OverlayExtent, limit: OverlayLimit) -> Self {
        if let OverlayGeometry::Edge {
            extent: e,
            limit: l,
            ..
        } = &mut self.geometry
        {
            *e = extent;
            *l = limit;
        }
        self
    }

    /// Set which corners round.
    pub fn corners(mut self, corners: OverlayCorners) -> Self {
        self.corners = corners;
        self
    }

    /// Set which edges carry an outline.
    pub fn border(mut self, border: OverlayBorder) -> Self {
        self.border = border;
        self
    }

    /// Set the panel's container rung.
    pub fn container(mut self, container: OverlayContainer) -> Self {
        self.container = container;
        self
    }

    /// Set the panel's elevation rung.
    pub fn elevation(mut self, elevation: OverlayElevation) -> Self {
        self.elevation = elevation;
        self
    }

    /// Set the entrance.
    pub fn entrance(mut self, entrance: OverlayEntrance) -> Self {
        self.entrance = entrance;
        self
    }

    /// Enable or disable scrim-tap dismissal.
    pub fn scrim_dismiss(mut self, scrim_dismiss: bool) -> Self {
        self.scrim_dismiss = scrim_dismiss;
        self
    }

    /// Show or hide the close affordance.
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.close_button = close_button;
        self
    }

    /// Show or hide the drag handle.
    pub fn handle(mut self, handle: bool) -> Self {
        self.handle = handle;
        self
    }

    /// Enable or disable drag-to-close (see [`OverlayModalConfig::drag`]).
    pub fn drag(mut self, drag: bool) -> Self {
        self.drag = drag;
        self
    }

    /// Set the accessibility role.
    pub fn role(mut self, role: OverlayRole) -> Self {
        self.role = role;
        self
    }
}

/// A view-held, typed dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A state-free close hook, fired from `paint` when an exit ramp settles (see
/// the [module docs](self)).
pub type OnClose = Rc<dyn Fn()>;

/// A modal component that [`show_overlay_modal`] can wire a navigator pop into.
///
/// A modal component implements it by storing the callback in its own
/// `on_dismiss` slot and reporting its own `dismissable` flag; the trait exists
/// so one push helper serves every family rather than each component
/// re-deriving the same `push_with_options` call.
///
/// Its element is the shared [`OverlayModalWidget`] by definition — a modal
/// component *is* a pre-configured modal host — which is what lets
/// [`show_overlay_modal`] install the staged-exit close hook and the back-press
/// dismiss signal on whatever component it was handed, with no per-component
/// setter.
pub trait OverlayModalContent<State: 'static>:
    View<State, Element = OverlayModalWidget> + Sized + 'static
{
    /// Install the dismiss callback, replacing any the builder already set.
    fn on_modal_dismiss(self, on_dismiss: OnDismiss<State>) -> Self;

    /// Whether this modal may be dismissed by the user at all — read once, at
    /// push time, to pick the page's [`BackPolicy`].
    fn modal_dismissable(&self) -> bool;
}

/// Build a modal host: `content` inside a panel shaped by `config`, over the M3
/// 32% scrim.
///
/// The content view owns all of the panel's padding and layout — the panel
/// itself contributes only chrome (fill, outline, corners, elevation) plus the
/// optional close affordance and drag handle, which sit *over* the content.
pub fn overlay_modal<State: 'static, V: View<State>>(
    content: V,
    config: OverlayModalConfig,
) -> OverlayModalView<State> {
    OverlayModalView {
        content: any(content),
        config,
        label: None,
        dismissable: true,
        on_dismiss: None,
        on_close: None,
        snap_points: Vec::new(),
    }
}

/// A declarative modal host. See [`overlay_modal`].
///
/// The fields are crate-visible so the catalog's modal components can wrap one
/// and adjust its config in their own builders (a dialog *is* a pre-configured
/// modal host); an app outside the crate configures it through
/// [`overlay_modal`]'s `config` argument and the builders below.
pub struct OverlayModalView<State: 'static> {
    pub(crate) content: AnyView<State>,
    pub(crate) config: OverlayModalConfig,
    pub(crate) label: Option<String>,
    pub(crate) dismissable: bool,
    pub(crate) on_dismiss: Option<OnDismiss<State>>,
    pub(crate) on_close: Option<OnClose>,
    pub(crate) snap_points: Vec<f64>,
}

impl<State: 'static> OverlayModalView<State> {
    /// Label the modal's accessibility node (its title text).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Whether the user can dismiss this modal at all — the scrim tap, the
    /// close affordance, the handle, a drag commit, `Escape`, and an Android
    /// back press (default `true`; see the [module docs](self)). `false`
    /// disables all six; only an explicit control the app wires through its own
    /// content still dismisses it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the **unstaged** dismiss callback — delivered with `&mut State`
    /// during the event pass.
    ///
    /// Used only when no [`on_close`](Self::on_close) hook is wired: with one,
    /// the dismissal is staged behind the exit ramp and fired from `paint`,
    /// where no app state exists (see the [module docs](self)).
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Set the state-free close hook fired when the exit ramp settles — the
    /// staged-dismissal path. [`show_overlay_modal`] wires this to
    /// `controller.pop()`; a `Stack`-mounted modal wires its own (a signal
    /// write, a navigator pop).
    ///
    /// Pinned at `build`: a directly constructed (non-staged) view's later
    /// rebuilds neither update nor clear this hook once installed — see
    /// [`OverlayModalWidget`]'s `on_close` field doc for why.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// Set the drag's snap points: `0..=1` a fraction of the panel's own
    /// extent, above `1` logical px (see the [module docs](self)). The panel
    /// opens to the first point — **except** a first point of `0.0` itself,
    /// which [`OverlayModalWidget::open_progress`] treats as absent (fully
    /// open) rather than authoring a panel that opens already closed.
    ///
    /// A non-finite point (NaN or infinite — never a meaningful fraction or
    /// px extent) is dropped here, at the boundary, rather than stored: this
    /// is the one place caller input enters the widget, and every downstream
    /// consumer (`normalize_snap`, `snap_targets`, `open_progress`) trusts
    /// [`OverlayModalWidget::snap_points`] to already be all-finite rather
    /// than re-checking (see the [module docs](self)'s Clamp Discipline
    /// section). A caller-authored NaN/∞ behaves as if that point were never
    /// passed at all.
    pub fn snap_points(mut self, points: &[f64]) -> Self {
        self.snap_points = points.iter().copied().filter(|p| p.is_finite()).collect();
        self
    }
}

impl<State: 'static> OverlayModalContent<State> for OverlayModalView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: OnDismiss<State>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.dismissable
    }
}

/// Push `build`'s modal as a transparent navigator page and register
/// `on_result` for the value it pops with — the shared push behind every modal
/// component's `show_*` wrapper.
///
/// The modal's dismissal is wired to `controller.pop()` (an *empty*
/// [`PopResult`], overriding any `on_dismiss` the builder set); an action inside
/// the content pops with a value via `controller.pop_with_result(..)`. The
/// navigator transition is [`TransitionSpec::NONE`] on purpose: the modal stages
/// its own entrance *and exit* ([`OverlayEntrance`]), so a page transition on
/// top of it would animate the same thing twice.
///
/// The back-press policy is peeked once, here, from
/// [`OverlayModalContent::modal_dismissable`] — the push-time `PushOptions`
/// contract fixes it for the life of the page, exactly as
/// [`crate::sheet`]'s own `show_bottom_sheet` peek does, even though `build` is
/// re-invoked on every later navigator rebuild to diff the page's content.
///
/// The pop is wired **twice, by design**: as the staged
/// [`OverlayModalView::on_close`] hook the exit ramp fires on settle (the path
/// every dismissal actually takes here), and as the unstaged
/// [`OverlayModalContent::on_modal_dismiss`] callback, which only runs for a
/// config with no entrance to reverse. Exactly one of the two fires per
/// dismissal.
pub fn show_overlay_modal<State, V, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    V: OverlayModalContent<State>,
    B: Fn() -> V + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss_ctrl = controller.clone();
    let close_ctrl = controller.clone();
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
            let ctrl = dismiss_ctrl.clone();
            let close = close_ctrl.clone();
            any::<State, _>(StagedExit {
                inner: build().on_modal_dismiss(Rc::new(move |_state: &mut State| ctrl.pop())),
                on_close: Rc::new(move || close.pop()) as OnClose,
                dismiss_signal: widget_signal.clone(),
            })
        },
        options,
    );
}

/// A modal component with the staged-exit close hook and the back-press
/// dismiss-signal cell installed on the [`OverlayModalWidget`] it builds.
///
/// [`show_overlay_modal`] is generic over every modal component, so it cannot
/// reach the [`OverlayModalView`] each of them wraps privately; every one of
/// them *does* build the shared [`OverlayModalWidget`]
/// ([`OverlayModalContent`]'s element bound), so this thin pass-through installs
/// both on the built widget instead — the same place [`OverlayModalView`]'s own
/// `on_close` lands. [`OverlayModalView::rebuild`] never writes either field
/// itself, so `StagedExit::rebuild`'s own reinstall below (which runs *after*
/// `inner.rebuild`) is the sole rebuild-time writer on this path — it always
/// wins rather than merely winning last.
struct StagedExit<V> {
    inner: V,
    on_close: OnClose,
    dismiss_signal: Option<Rc<Cell<u64>>>,
}

impl<State: 'static, V: View<State, Element = OverlayModalWidget>> View<State> for StagedExit<V> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        let mut widget = self.inner.build(ctx);
        widget.on_close = Some(self.on_close.clone());
        // Seeded from the cell's *current* generation: a bump that predates
        // this widget is not a back press aimed at it.
        widget.last_seen_dismiss = self.dismiss_signal.as_ref().map_or(0, |s| s.get());
        widget.dismiss_signal = self.dismiss_signal.clone();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = self.inner.rebuild(&prev.inner, element, ctx);
        element.on_close = Some(self.on_close.clone());
        // The cell's identity is fixed at push time, so reinstalling it never
        // disturbs `last_seen_dismiss`.
        element.dismiss_signal = self.dismiss_signal.clone();
        flags
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        self.inner.teardown(element, ctx);
    }
}

/// The retained widget for an [`OverlayModalView`].
pub struct OverlayModalWidget {
    content: ChildPod,
    config: OverlayModalConfig,
    label: Option<String>,
    dismissable: bool,
    on_dismiss: Option<ErasedCallback>,
    /// The state-free close hook the exit ramp fires on settle (see the module
    /// docs); `None` leaves dismissal unstaged.
    ///
    /// Installed at [`OverlayModalView::build`] and, on the staged navigator
    /// path, reinstalled after every rebuild by [`StagedExit`] — the sole
    /// rebuild-time writer. [`OverlayModalView::rebuild`] itself never touches
    /// this field: a directly constructed, non-staged view (one no `StagedExit`
    /// wraps) therefore has its `on_close` pinned at whatever `build` installed
    /// for the widget's whole mounted lifetime — a later rebuild neither updates
    /// nor clears it, since only the staged wrapper needs a live reinstall (its
    /// own hook is a fixed `controller.pop()` closure that never changes across
    /// rebuilds anyway).
    on_close: Option<OnClose>,
    /// The shared back-press dismiss-signal cell (the `DismissAnimated` seam;
    /// see [`Self::observe_dismiss_signal`]), installed by [`StagedExit`].
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last generation observed from `dismiss_signal` (0 with no signal
    /// wired — a `Stack` mount, or a `Veto` page).
    last_seen_dismiss: u64,
    /// The drag's resting points, as authored (see
    /// [`OverlayModalView::snap_points`]).
    snap_points: Vec<f64>,
    /// The panel rect in the widget's own coordinate space (computed at layout,
    /// hit-tested at event time).
    panel: Rect,
    /// The ramp driver, the endpoints it interpolates between, and the progress
    /// `layout` last used: `progress = from + (to − from) · anim.value()`.
    anim: AnimationController,
    ramp: (f64, f64),
    progress: f64,
    /// Whether the first paint has seeded the ramp (see the module docs'
    /// `reduce_motion` note).
    started: bool,
    /// Whether the running ramp ends in a dismissal.
    exiting: bool,
    /// One-shot latch set the instant `on_close` fires (settle, or the
    /// immediate no-ramp path in [`Self::request_dismiss`]). Terminal for this
    /// widget's mounted lifetime: the hook only *enqueues* a navigator pop, so
    /// the widget stays mounted and still a barrier for one or more frames
    /// after firing, during which a further dismiss trigger must not re-fire it
    /// — that would pop whatever page is now underneath this one.
    /// `request_dismiss` refuses every trigger once set; nothing ever clears it
    /// back to `false`.
    closed: bool,
    /// A scrim/panel-background press is in flight (the modal barrier).
    scrim_captured: bool,
    /// Whether that press started outside the panel — only an outside press
    /// released outside dismisses.
    scrim_down_outside: bool,
    /// The close affordance's latched hover/press state.
    close_hovered: bool,
    close_captured: bool,
    /// The drag handle's latched hover/press state.
    handle_hovered: bool,
    handle_captured: bool,
    /// A drag along the panel's own axis is in flight, where it started (axis
    /// position and the progress it began from), whether it began on the handle,
    /// and whether it has passed [`TOUCH_SLOP`] into a real drag.
    drag_captured: bool,
    drag_start: f64,
    drag_from: f64,
    drag_on_handle: bool,
    drag_moved: bool,
    /// The release-velocity estimator, clocked from the last painted frame —
    /// pointer events carry no timestamp of their own (`frust_widgets::scroll`'s
    /// precedent).
    tracker: VelocityTracker,
    last_frame_time: FrameTime,
}

impl OverlayModalWidget {
    /// The panel rect in the widget's own coordinate space.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    /// The close affordance's hit box, when the config shows one.
    pub fn close_rect(&self) -> Option<Rect> {
        self.config.close_button.then(|| {
            let center = Point::new(
                self.panel.x1 - CLOSE_INSET - CLOSE_ICON / 2.0,
                self.panel.y0 + CLOSE_INSET + CLOSE_ICON / 2.0,
            );
            Rect::from_center_size(center, Size::new(CLOSE_HIT, CLOSE_HIT))
        })
    }

    /// The drag handle's hit box, when the config shows one.
    pub fn handle_rect(&self) -> Option<Rect> {
        self.config
            .handle
            .then(|| Rect::from_center_size(self.handle_center(), Size::new(CLOSE_HIT, CLOSE_HIT)))
    }

    /// The centre of the handle indicator, in the widget's own space.
    fn handle_center(&self) -> Point {
        Point::new(
            self.panel.center().x,
            self.panel.y0 + HANDLE_TOUCH_TARGET / 2.0,
        )
    }

    /// The handle's painted bar (a subset of its hit box — see [`CLOSE_HIT`]).
    fn handle_bar(&self) -> Rect {
        Rect::from_center_size(self.handle_center(), Size::new(HANDLE_WIDTH, HANDLE_HEIGHT))
    }

    /// The ramp's progress: `0.0` off-screen/transparent, `1.0` fully shown.
    pub fn progress(&self) -> f64 {
        self.progress
    }

    /// Whether an exit ramp is running (the panel is on its way out, and the
    /// barrier is still swallowing input).
    pub fn is_exiting(&self) -> bool {
        self.exiting
    }

    /// The progress an open panel rests at: the first snap point, or fully open
    /// with none authored.
    ///
    /// Floored above [`PROGRESS_EPSILON`]: a first point that normalizes to
    /// `0.0` (an explicit `snap_points(&[0.0])`, or a `0.0`-fraction point on an
    /// as-yet-unmeasured extent) is treated as **absent** rather than as "open
    /// to closed" — the entrance ramp always has a nonzero target to animate
    /// toward, matching [`Self::settle_drag`]'s own floor for a panel that
    /// cannot dismiss. There is no fallible surface here
    /// ([`OverlayModalView::snap_points`] is a plain builder call), so an
    /// unreachable configuration silently falls back to fully open instead of
    /// panicking.
    pub fn open_progress(&self) -> f64 {
        self.snap_points
            .first()
            .map(|p| self.normalize_snap(*p))
            .filter(|p| *p > PROGRESS_EPSILON)
            .unwrap_or(1.0)
    }

    /// A snap point in progress space: `0..=1` is already a fraction of the
    /// panel's extent, anything larger is logical px against that extent.
    /// Clamped into `0..=1`, and treated as fully open while no extent has been
    /// laid out yet.
    fn normalize_snap(&self, point: f64) -> f64 {
        if point <= 1.0 {
            return point.clamp(0.0, 1.0);
        }
        let extent = self.extent();
        if extent <= 0.0 {
            return 1.0;
        }
        (point / extent).clamp(0.0, 1.0)
    }

    /// The resting points a release snaps to, ascending: every snap point plus
    /// `0.0` (closed). With none authored the set is `{0.0, 1.0}`, so "nearest"
    /// is the drag-past-the-midpoint commit.
    fn snap_targets(&self) -> Vec<f64> {
        let mut targets: Vec<f64> = std::iter::once(0.0)
            .chain(self.snap_points.iter().map(|p| self.normalize_snap(*p)))
            .collect();
        if targets.len() == 1 {
            targets.push(1.0);
        }
        targets.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        targets
    }

    /// The panel's extent along its own slide axis (its thickness).
    fn extent(&self) -> f64 {
        match self.config.geometry {
            OverlayGeometry::Edge { side, .. } if side.is_vertical() => self.panel.height(),
            OverlayGeometry::Edge { .. } => self.panel.width(),
            OverlayGeometry::Centered { .. } => 0.0,
        }
    }

    /// The panel's slide axis, and which way along it closes: `+1.0` when the
    /// panel leaves toward growing coordinates (bottom/right), `-1.0` otherwise.
    fn close_sign(&self) -> f64 {
        match self.config.geometry {
            OverlayGeometry::Edge {
                side: OverlaySide::Bottom | OverlaySide::Right,
                ..
            } => 1.0,
            _ => -1.0,
        }
    }

    /// `position` projected onto the panel's slide axis.
    fn axis_pos(&self, position: Point) -> f64 {
        match self.config.geometry {
            OverlayGeometry::Edge { side, .. } if side.is_vertical() => position.y,
            _ => position.x,
        }
    }

    /// The last painted frame time in milliseconds — the event pass's clock,
    /// since a pointer event carries none.
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Whether a dismissal can actually be delivered: the user-facing
    /// [`dismissable`](OverlayModalView::dismissable) flag *and* a hook to fire.
    fn can_dismiss(&self) -> bool {
        self.dismissable && (self.on_dismiss.is_some() || self.on_close.is_some())
    }

    /// Start a ramp from the current progress to `to`, over the entrance's own
    /// duration scaled by how much of the travel is left, eased by its own
    /// direction's curve.
    fn begin_ramp(&mut self, to: f64, exiting: bool) {
        let from = self.progress;
        self.ramp = (from, to);
        self.exiting = exiting;
        let fraction = (to - from).abs().clamp(0.0, 1.0);
        let duration = self.config.entrance.duration().mul_f64(fraction);
        self.anim =
            AnimationController::new(duration).with_curve(self.config.entrance.curve(exiting));
        self.anim.forward();
        self.started = true;
    }

    /// The progress the running ramp is at.
    fn ramp_value(&self) -> f64 {
        let (from, to) = self.ramp;
        from + (to - from) * self.anim.value_clamped()
    }

    /// Ask for the pass a moving ramp needs: a relayout when the motion moves
    /// the panel's geometry (`request_layout` implies a frame), a bare frame
    /// otherwise.
    fn request_continuation(&self, ctx: &mut PaintCtx) {
        if self.config.entrance.is_layout_affecting() {
            ctx.request_layout();
        } else {
            ctx.request_frame();
        }
    }

    /// Whether a staged exit is possible at all: something to fire from `paint`,
    /// and an entrance to reverse.
    fn stageable(&self) -> bool {
        self.on_close.is_some() && self.config.entrance != OverlayEntrance::None
    }

    /// Stage the dismissal: run the entrance ramp backwards and fire the close
    /// hook when it settles.
    ///
    /// Falls back to firing [`OverlayModalView::on_dismiss`] on the spot when
    /// there is nothing to stage — no close hook to fire from `paint`, or no
    /// entrance to reverse. A trigger arriving while an exit is already running,
    /// after `on_close` has already fired once ([`Self::closed`]), or on a
    /// non-dismissable modal is a no-op — the `closed` latch guards a trigger
    /// landing between the enqueued pop and the rebuild that drains it, when the
    /// widget is still mounted and would otherwise stage (or immediately fire) a
    /// second, spurious close.
    fn request_dismiss(&mut self, ctx: &mut EventCtx) {
        if self.exiting || self.closed || !self.dismissable {
            return;
        }
        if !self.stageable() {
            if let Some(on_dismiss) = self.on_dismiss.as_mut() {
                on_dismiss(ctx);
            } else if let Some(on_close) = &self.on_close {
                self.closed = true;
                on_close();
            }
            return;
        }
        self.begin_ramp(0.0, true);
        ctx.request_redraw();
    }

    /// Observe the shared back-press dismiss-signal cell and stage the exit
    /// exactly once per bump — the [`BackPolicy::DismissAnimated`] seam's
    /// widget-side half (see the module docs' back-dismiss section). A back
    /// request flags `PAINT`, so this always runs before the next frame is
    /// shown.
    ///
    /// The paint-pass twin of [`Self::request_dismiss`]: identical guards,
    /// except that the unstaged fallback can only reach the state-free
    /// `on_close` (no `&mut State` exists here), and firing it must itself
    /// request the frame whose rebuild drains the enqueued pop — the drain
    /// gotcha the module docs spell out.
    fn observe_dismiss_signal(&mut self, ctx: &mut PaintCtx) {
        let Some(signal) = &self.dismiss_signal else {
            return;
        };
        let current = signal.get();
        if current == self.last_seen_dismiss {
            return;
        }
        self.last_seen_dismiss = current;
        if self.exiting || self.closed || !self.dismissable {
            return;
        }
        if !self.stageable() {
            if let Some(on_close) = &self.on_close {
                self.closed = true;
                on_close();
                ctx.request_frame();
            }
            return;
        }
        self.begin_ramp(0.0, true);
        self.request_continuation(ctx);
    }

    /// Move the panel to `progress` without animating — the drag scrub. The
    /// panel keeps the extent layout gave it and only its origin moves, so no
    /// relayout is owed.
    ///
    /// A live drag takes the panel over from whatever ramp was running,
    /// including an exit: catching a closing sheet cancels its dismissal, and
    /// the release decides again from where the finger left it.
    fn scrub(&mut self, ctx: &mut EventCtx, progress: f64) {
        if (progress - self.progress).abs() < PROGRESS_EPSILON {
            return;
        }
        self.anim.stop();
        self.exiting = false;
        self.progress = progress;
        self.ramp = (progress, progress);
        self.reposition(ctx.size());
        ctx.request_redraw();
    }

    /// Re-place an edge-pinned panel (and its content) for the current progress,
    /// inside an area of `area`.
    fn reposition(&mut self, area: Size) {
        let OverlayGeometry::Edge { side, .. } = self.config.geometry else {
            return;
        };
        let size = self.panel.size();
        let origin = edge_origin(side, area, size, self.progress);
        self.panel = Rect::from_origin_size(origin, size);
        self.content.set_origin(origin);
    }

    /// Open a drag from `position`, remembering where along the axis it started
    /// and what progress it started from.
    fn begin_drag(&mut self, ctx: &mut EventCtx, position: Point, on_handle: bool) {
        self.drag_captured = true;
        self.drag_on_handle = on_handle;
        self.drag_moved = false;
        self.drag_start = self.axis_pos(position);
        self.drag_from = self.progress;
        self.tracker.clear();
        self.tracker.record(self.event_time_ms(), self.drag_start);
        ctx.capture_pointer();
    }

    /// Track a captured drag: sample the velocity, and once the gesture has
    /// passed [`TOUCH_SLOP`] scrub the panel to follow the pointer.
    ///
    /// The scrub measures from the `Down`, slop included — unlike
    /// `frust_widgets::scroll`, which re-baselines when it *takes the gesture
    /// over* from a child. Nothing is taken over here (the panel captured the
    /// press outright; the slop only separates a tap from a drag), so
    /// re-baselining would leave the panel trailing the finger for the rest of
    /// the gesture.
    fn drag_move(&mut self, ctx: &mut EventCtx, position: Point) {
        let axis = self.axis_pos(position);
        self.tracker.record(self.event_time_ms(), axis);
        let delta = axis - self.drag_start;
        if !self.drag_moved && delta.abs() > TOUCH_SLOP {
            self.drag_moved = true;
        }
        if !self.drag_moved {
            return;
        }
        let extent = self.extent();
        if extent <= 0.0 {
            return;
        }
        // Dragging toward the closing edge lowers progress; the panel can be
        // pushed no further open than its own furthest resting point.
        let open = self.snap_targets().last().copied().unwrap_or(1.0);
        let target = self.drag_from - self.close_sign() * delta / extent;
        // Ordered explicitly rather than `clamp`: `open` is computed from the
        // live snap targets, and clamp bounds must be provably ordered and
        // finite (see the [module docs](self)'s Clamp Discipline section).
        // `f64::max`/`f64::min` also can't panic the way `clamp`'s internal
        // `assert!(min <= max)` would if a NaN ever reached `open`.
        self.scrub(ctx, target.max(0.0).min(open));
    }

    /// The resting point a release settles to: the nearest one, or — past
    /// [`OVERLAY_FLING_VELOCITY`] — the next one along the direction of travel.
    fn release_target(&self, velocity: f64) -> f64 {
        let targets = self.snap_targets();
        let nearest = |p: f64| {
            targets
                .iter()
                .copied()
                .min_by(|a, b| {
                    (a - p)
                        .abs()
                        .partial_cmp(&(b - p).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0.0)
        };
        if velocity.abs() >= OVERLAY_FLING_VELOCITY {
            // A flick toward the closing edge takes the next point down, one
            // away from it the next point up.
            let opening = velocity * self.close_sign() < 0.0;
            let next = if opening {
                targets.iter().copied().find(|t| *t > self.progress)
            } else {
                targets.iter().rev().copied().find(|t| *t < self.progress)
            };
            if let Some(next) = next {
                return next;
            }
        }
        nearest(self.progress)
    }

    /// Settle a released drag: continue into the exit ramp when it landed
    /// closed, otherwise ramp back to the point it snapped to.
    ///
    /// A closed landing on a panel that cannot dismiss springs back to
    /// [`Self::open_progress`] instead of staging an exit:
    /// [`Self::request_dismiss`] no-ops there, and by the time a drag settles
    /// `progress` has already tracked the pointer down near `0.0` (unlike the
    /// tap-to-dismiss paths, which never move it) — a bare no-op would leave the
    /// panel resting there, an invisible full-window input barrier with nothing
    /// to recover it.
    fn settle_drag(&mut self, ctx: &mut EventCtx, target: f64) {
        if target <= PROGRESS_EPSILON {
            if self.can_dismiss() {
                self.request_dismiss(ctx);
            } else {
                self.begin_ramp(self.open_progress(), false);
                ctx.request_redraw();
            }
        } else {
            self.begin_ramp(target, false);
            ctx.request_redraw();
        }
    }
}

impl<State: 'static> View<State> for OverlayModalView<State> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        let settled = self.config.entrance == OverlayEntrance::None;
        OverlayModalWidget {
            content: build_child(&self.content, ctx),
            config: self.config,
            label: self.label.clone(),
            dismissable: self.dismissable,
            on_dismiss: self.on_dismiss.as_ref().map(erase_callback),
            on_close: self.on_close.clone(),
            dismiss_signal: None,
            last_seen_dismiss: 0,
            snap_points: self.snap_points.clone(),
            panel: Rect::ZERO,
            anim: AnimationController::new(self.config.entrance.duration())
                .with_curve(self.config.entrance.curve(false)),
            // The entrance's target resolves on the first paint, not here: a
            // snap point in logical px needs the extent layout has yet to
            // measure.
            ramp: (0.0, 1.0),
            progress: if settled { 1.0 } else { 0.0 },
            started: settled,
            exiting: false,
            closed: false,
            scrim_captured: false,
            scrim_down_outside: false,
            close_hovered: false,
            close_captured: false,
            handle_hovered: false,
            handle_captured: false,
            drag_captured: false,
            drag_start: 0.0,
            drag_from: 0.0,
            drag_on_handle: false,
            drag_moved: false,
            tracker: VelocityTracker::new(),
            last_frame_time: FrameTime::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.config != self.config {
            element.config = self.config;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        if element.dismissable != self.dismissable {
            element.dismissable = self.dismissable;
        }
        if element.snap_points != self.snap_points {
            element.snap_points = self.snap_points.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // `on_dismiss` is reinstalled unconditionally (cheap — closures aren't
        // comparable, so this is what every interactive widget does).
        // `on_close` and `dismiss_signal` are deliberately left untouched here —
        // see `OverlayModalWidget::on_close`'s field doc for why.
        element.on_dismiss = self.on_dismiss.as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

/// Lay the panel out and place the content inside it.
///
/// Split out of [`Widget::layout`] so the geometry is one readable expression
/// per [`OverlayGeometry`] arm; returns the panel rect (already offset by the
/// slide progress, if any).
fn layout_panel(
    content: &mut ChildPod,
    ctx: &mut LayoutCtx,
    config: OverlayModalConfig,
    area: Size,
    progress: f64,
) -> Rect {
    match config.geometry {
        OverlayGeometry::Centered { max_width } => {
            let available = (area.width - 2.0 * CENTERED_MARGIN).max(0.0);
            // The M3 [280, 560] clamp, with the viewport margin winning on a
            // window too narrow for the minimum.
            let width = max_width
                .min(available)
                .max(OVERLAY_DIALOG_MIN_WIDTH.min(available));
            let max_height = (area.height - 2.0 * CENTERED_MARGIN).max(0.0);
            // A tight width, a loose height: the panel is as tall as its
            // content, up to the area.
            let measured = content.layout_child(
                ctx,
                &BoxConstraints::new(Size::new(width, 0.0), Size::new(width, max_height)),
            );
            let height = measured.height.min(max_height);
            let origin = Point::new(
                ((area.width - width) / 2.0).max(0.0),
                ((area.height - height) / 2.0).max(0.0),
            );
            let panel = Rect::from_origin_size(origin, Size::new(width, height));
            content.set_origin(panel.origin());
            panel
        }
        OverlayGeometry::Edge {
            side,
            extent,
            limit,
        } => {
            let (axis_area, cross) = if side.is_vertical() {
                (area.height, area.width)
            } else {
                (area.width, area.height)
            };
            let cap = limit.apply(axis_area, axis_area);
            let thickness = match extent {
                OverlayExtent::Fraction(f) => limit.apply(axis_area * f, axis_area),
                OverlayExtent::Content => {
                    let bc = if side.is_vertical() {
                        BoxConstraints::new(Size::new(cross, 0.0), Size::new(cross, cap))
                    } else {
                        BoxConstraints::new(Size::new(0.0, cross), Size::new(cap, cross))
                    };
                    let measured = content.layout_child(ctx, &bc);
                    let along = if side.is_vertical() {
                        measured.height
                    } else {
                        measured.width
                    };
                    along.min(cap)
                }
            };
            let size = if side.is_vertical() {
                Size::new(cross, thickness)
            } else {
                Size::new(thickness, cross)
            };
            if extent != OverlayExtent::Content {
                content.layout_child(ctx, &BoxConstraints::tight(size));
            }
            let origin = edge_origin(side, area, size, progress);
            let panel = Rect::from_origin_size(origin, size);
            content.set_origin(panel.origin());
            panel
        }
    }
}

/// Where an edge-pinned panel of `size` sits inside `area` at `progress`: at
/// `0.0` entirely outside its own edge, at `1.0` flush against it, in between
/// the fraction of it the slide (or a drag) has brought in.
///
/// Shared by [`layout_panel`] and the drag scrub, which moves the panel between
/// layout passes and must place it identically.
fn edge_origin(side: OverlaySide, area: Size, size: Size, progress: f64) -> Point {
    let thickness = if side.is_vertical() {
        size.height
    } else {
        size.width
    };
    let out = thickness * (1.0 - progress);
    match side {
        OverlaySide::Top => Point::new(0.0, -out),
        OverlaySide::Bottom => Point::new(0.0, area.height - thickness + out),
        OverlaySide::Left => Point::new(-out, 0.0),
        OverlaySide::Right => Point::new(area.width - thickness + out, 0.0),
    }
}

/// Stroke the panel's outline per [`OverlayBorder`].
fn paint_border(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    border: OverlayBorder,
    color: Color,
) {
    let half = BORDER_WIDTH / 2.0;
    let (from, to) = match border {
        OverlayBorder::None => return,
        OverlayBorder::All => {
            // Inset by half the stroke width so the hairline paints fully
            // inside the panel's own bounds (a stroke is centred on its path) —
            // the `crate::card` outlined precedent.
            let mut path = BezPath::new();
            path.move_to(Point::new(half, half));
            path.line_to(Point::new(size.width - half, half));
            path.line_to(Point::new(size.width - half, size.height - half));
            path.line_to(Point::new(half, size.height - half));
            path.close_path();
            scene.stroke_path(origin, &path, BORDER_WIDTH, &Brush::Solid(color));
            return;
        }
        OverlayBorder::Edge(OverlaySide::Top) => {
            (Point::new(0.0, half), Point::new(size.width, half))
        }
        OverlayBorder::Edge(OverlaySide::Bottom) => (
            Point::new(0.0, size.height - half),
            Point::new(size.width, size.height - half),
        ),
        OverlayBorder::Edge(OverlaySide::Left) => {
            (Point::new(half, 0.0), Point::new(half, size.height))
        }
        OverlayBorder::Edge(OverlaySide::Right) => (
            Point::new(size.width - half, 0.0),
            Point::new(size.width - half, size.height),
        ),
    };
    let mut path = BezPath::new();
    path.move_to(from);
    path.line_to(to);
    scene.stroke_path(origin, &path, BORDER_WIDTH, &Brush::Solid(color));
}

/// Return `color` with its own alpha scaled by `factor` — how the scrim rides
/// the ramp.
fn scale_alpha(color: Color, factor: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * factor])
}

/// Whether `pos` falls inside an optional hit box (`false` when the box is not
/// shown at all).
fn hit(rect: Option<Rect>, pos: Point) -> bool {
    rect.is_some_and(|r| r.contains(pos))
}

impl Widget for OverlayModalWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        self.panel = layout_panel(&mut self.content, ctx, self.config, area, self.progress);
        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The back-press seam, first: a bump staged here rides the very same
        // ramp the pointer/key gestures below drive.
        self.observe_dismiss_signal(ctx);
        // Authoritative hover read: a pointer that left this widget sends it no
        // event at all, so the chrome's latched flags are cleared whenever the
        // hover path no longer runs through here. It cannot *set* them — the
        // path is true for a hovered content child too — which is what the
        // per-region latches in `event` are for.
        if !ctx.is_hovered() {
            self.close_hovered = false;
            self.handle_hovered = false;
        }
        let (origin, size) = (ctx.origin(), ctx.size());
        let now = ctx.frame_time();
        // Every theme read in one scope: the `&Theme` borrows the context, and
        // the child paint plus the frame requests below need it mutably.
        let (reduce, radius, scrim, fill, border, ink, handle_color, shadow) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                reduce_motion(theme),
                super::radius(theme),
                super::scrim(theme),
                super::container(theme, self.config.container),
                super::outline_variant(theme),
                super::on_surface(theme),
                super::on_surface_variant(theme),
                super::shadow(theme, self.config.elevation),
            )
        };

        // The event pass has no clock of its own; this is the one it reads.
        self.last_frame_time = now;

        // Advance (or collapse) the running ramp. The entrance's target
        // resolves here, on the first paint, because a snap point in logical px
        // needs the extent the layout before it measured.
        if !self.started {
            self.started = true;
            self.ramp = (0.0, self.open_progress());
            if !reduce {
                self.anim.forward();
            }
        }
        let next = if reduce {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.ramp.1
        } else if self.anim.is_animating() {
            // `advance`'s first call after `forward()` only seeds the clock
            // (zero delta, but still truthy) — the continuation has to be
            // requested on every truthy advance, not just the ones that moved
            // `progress`, or a slide's seeding paint never schedules the frame
            // that would carry it off zero.
            if self.anim.advance(now) {
                self.request_continuation(ctx);
            }
            self.ramp_value()
        } else {
            self.progress
        };
        if (next - self.progress).abs() > PROGRESS_EPSILON {
            self.progress = next;
            self.request_continuation(ctx);
        }
        // A settled exit is where the deferred dismissal is finally fired. The
        // hook only *enqueues* a navigator pop (it writes no tracked signal), so
        // this paint asks for the frame whose rebuild drains it — otherwise a
        // `ControlFlow::Wait` desktop shell idles and the modal never leaves.
        if self.exiting && !self.anim.is_animating() && self.progress <= PROGRESS_EPSILON {
            self.exiting = false;
            self.progress = 0.0;
            if let Some(on_close) = &self.on_close {
                self.closed = true;
                on_close();
                ctx.request_frame();
            }
        }
        let progress = self.progress;

        // The scrim fades with the panel, and covers the whole area.
        scene.fill_rect(origin, size, scale_alpha(scrim, progress as f32));

        let panel_origin = Point::new(origin.x + self.panel.x0, origin.y + self.panel.y0);
        let panel_size = self.panel.size();
        let radii = self.config.corners.radii(radius);
        let fade_scale = self.config.entrance == OverlayEntrance::FadeScale && progress < 1.0;
        if fade_scale {
            // Composite the panel at the ramp's alpha. The layer rect is
            // inflated so the elevation shadow, which reaches outside the panel,
            // is not clipped out of it.
            let layer = Rect::from_origin_size(panel_origin, panel_size)
                .inflate(SHADOW_SPILL, SHADOW_SPILL);
            scene.push_layer(layer.origin(), layer.size(), progress as f32);
            // …and scale about the panel's own centre.
            let c = Rect::from_origin_size(panel_origin, panel_size).center();
            let scale = SCALE_FROM + (1.0 - SCALE_FROM) * progress;
            scene.push_transform(
                Affine::translate(c.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-c.to_vec2()),
            );
        }

        if let Some((blur, y_offset, color)) = shadow {
            scene.draw_shadow(
                Point::new(panel_origin.x, panel_origin.y + y_offset),
                panel_size,
                radii.largest(),
                blur,
                color,
            );
        }
        scene.fill_rounded_rect_radii(panel_origin, panel_size, radii, fill);

        // The content is clipped to the panel: a scrolling panel needs it, and
        // it is what keeps a too-tall panel's content inside the window.
        scene.push_clip_rounded_radii(panel_origin, panel_size, radii);
        self.content.paint_child(ctx, scene);
        scene.pop_clip();

        paint_border(scene, panel_origin, panel_size, self.config.border, border);

        if self.config.close_button {
            // M3 has no rest-state opacity ramp on an icon (a state layer does
            // the hover work): the affordance sits in `onSurfaceVariant` and
            // steps up to `onSurface` when hovered — the source's
            // `opacity-70 hover:opacity-100`, re-expressed in roles.
            let color = if self.close_hovered {
                ink
            } else {
                handle_color
            };
            let (path, design) = IconData::from(crate::icons::CLOSE).resolve();
            let scale = if design > 0.0 {
                CLOSE_ICON / design
            } else {
                1.0
            };
            let icon_origin = Point::new(
                origin.x + self.panel.x1 - CLOSE_INSET - CLOSE_ICON,
                origin.y + self.panel.y0 + CLOSE_INSET,
            );
            scene.fill_path(
                icon_origin,
                &(Affine::scale(scale) * path),
                &Brush::Solid(color),
            );
        }
        if self.config.handle {
            let bar = self.handle_bar();
            scene.fill_rounded_rect(
                Point::new(origin.x + bar.x0, origin.y + bar.y0),
                bar.size(),
                bar.height() / 2.0,
                handle_color,
            );
        }

        if fade_scale {
            scene.pop_transform();
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Claim focus on every `Down` — the `crate::dialog`/`crate::sheet`
        // opt-in that makes Escape reachable, and what keeps the root's focus
        // session alive while the modal is up (re-claiming while focused is a
        // no-op; a claim-once guard would kill the session on the second tap).
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            ctx.request_focus();
        }
        // Content first: an action inside the panel owns its own events, and
        // this widget's own hover claims come after the routing — *unless* this
        // widget already holds the gesture. A capture recorded here (a barrier
        // press, a live drag) means the content declined the `Down` that opened
        // it, so re-routing its `Move`s would let a control the pointer happens
        // to travel over steal a drag mid-flight. Broadcasts and focus-routed
        // events are never short-circuited.
        let captured = self.close_captured
            || self.handle_captured
            || self.scrim_captured
            || self.drag_captured;
        let own_gesture = captured && matches!(event, InputEvent::Pointer(_));
        if !own_gesture && route_event_single(&mut self.content, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        if let InputEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Escape) {
                self.request_dismiss(ctx);
            }
            // The barrier swallows every other key too: nothing behind a modal
            // may act on a keystroke its content declined.
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        // A secondary press is swallowed like every other one — a modal blocks
        // the page behind it whatever button pressed — but it operates nothing:
        // no close press, no drag, no scrim-dismiss arming. Only the primary
        // button does any of that (the catalog's press rule), and the panel's
        // own content, routed above, has already had its chance at the event (a
        // context menu inside the panel still opens).
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Handled;
        }
        let (close, handle) = (self.close_rect(), self.handle_rect());
        match p.phase {
            PointerPhase::Move => {
                if self.drag_captured {
                    // A captured drag re-asks for its cursor from its own arm,
                    // so the shape survives the pointer leaving the panel.
                    ctx.set_cursor(CursorIcon::Grabbing);
                    self.drag_move(ctx, p.position);
                    return EventResult::Handled;
                }
                if self.close_captured || self.handle_captured || self.scrim_captured {
                    if self.close_captured || self.handle_captured {
                        ctx.set_cursor(CursorIcon::Pointer);
                    }
                    return EventResult::Handled;
                }
                let over_close = hit(close, p.position);
                let over_handle = hit(handle, p.position);
                if over_close || over_handle {
                    // Claimed *after* the content routing above.
                    ctx.claim_hover();
                    if over_handle && self.config.drag {
                        // A draggable handle advertises the gesture.
                        ctx.set_cursor(CursorIcon::Grab);
                    } else {
                        ctx.set_cursor(CursorIcon::Pointer);
                    }
                }
                if self.close_hovered != over_close {
                    self.close_hovered = over_close;
                    ctx.request_redraw();
                }
                if self.handle_hovered != over_handle {
                    self.handle_hovered = over_handle;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if hit(close, p.position) {
                    self.close_captured = true;
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                // A draggable panel takes the press as a drag — from the handle
                // or from the panel's own surface — and still resolves it as a
                // tap if it never moves.
                if self.config.drag && (hit(handle, p.position) || self.panel.contains(p.position))
                {
                    self.begin_drag(ctx, p.position, hit(handle, p.position));
                    return EventResult::Handled;
                }
                if hit(handle, p.position) {
                    self.handle_captured = true;
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                // The modal barrier: swallow, and remember whether the press
                // started outside the panel.
                self.scrim_captured = true;
                self.scrim_down_outside = !self.panel.contains(p.position);
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.close_captured {
                    self.close_captured = false;
                    if hit(close, p.position) {
                        self.request_dismiss(ctx);
                    }
                    return EventResult::Handled;
                }
                if self.handle_captured {
                    self.handle_captured = false;
                    if hit(handle, p.position) {
                        self.request_dismiss(ctx);
                    }
                    return EventResult::Handled;
                }
                if self.drag_captured {
                    self.drag_captured = false;
                    if !self.drag_moved {
                        // A tap, not a drag: on the handle it dismisses, on the
                        // panel it is an ordinary swallowed barrier press.
                        if self.drag_on_handle && hit(handle, p.position) {
                            self.request_dismiss(ctx);
                        }
                        return EventResult::Handled;
                    }
                    let target = self.release_target(self.tracker.velocity());
                    self.settle_drag(ctx, target);
                    return EventResult::Handled;
                }
                if !self.scrim_captured {
                    return EventResult::Ignored;
                }
                self.scrim_captured = false;
                let released_outside = !self.panel.contains(p.position);
                if self.config.scrim_dismiss && self.scrim_down_outside && released_outside {
                    self.request_dismiss(ctx);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // A `Cancel` arm never touches app state — flags only, plus the
                // ramp back to where the drag started.
                self.close_captured = false;
                self.handle_captured = false;
                self.scrim_captured = false;
                if self.drag_captured {
                    self.drag_captured = false;
                    if self.drag_moved {
                        self.begin_ramp(self.drag_from, false);
                        ctx.request_redraw();
                    }
                }
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        KeyEvent, Modifiers, PointerButton, PointerEvent, Shape, any as core_any,
    };
    use frust::{Brightness, NavigatorView};
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use std::any::Any;

    /// The window every modal test lays out in.
    const WINDOW: Size = Size::new(400.0, 600.0);

    /// A recording `PaintScene`: the fills, paths, strokes, clips, shadows and
    /// fade layers a modal emits.
    #[derive(Default)]
    struct Recorder {
        /// `fill_rect` calls — the scrim is the first of them.
        rects: Vec<(Point, Size, Color)>,
        /// `fill_rounded_rect_radii` calls — the panel.
        panels: Vec<(Point, Size, CornerRadii, Color)>,
        /// `fill_rounded_rect` calls — the drag handle.
        rrects: Vec<(Point, Size, f64, Color)>,
        /// `fill_path` calls, as `(origin, bounding box, color)` — the close
        /// icon.
        paths: Vec<(Point, Rect, Color)>,
        /// `stroke_path` calls, as `(bounding box, width, color)`.
        strokes: Vec<(Rect, f64, Color)>,
        /// `draw_shadow` calls.
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        /// `push_clip*` calls.
        clips: Vec<(Point, Size)>,
        /// `push_layer` alphas — the fade.
        layers: Vec<f32>,
        /// `push_transform` affines — the scale.
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn fill_rounded_rect_radii(
            &mut self,
            origin: Point,
            size: Size,
            radii: CornerRadii,
            color: Color,
        ) {
            self.panels.push((origin, size, radii, color));
        }
        fn fill_path(&mut self, origin: Point, path: &BezPath, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.paths.push((origin, path.bounding_box(), color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn draw_shadow(
            &mut self,
            origin: Point,
            size: Size,
            radius: f64,
            std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, size, radius, std_dev, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, _radius: f64) {
            self.clips.push((origin, size));
        }
        fn push_clip_rounded_radii(&mut self, origin: Point, size: Size, _radii: CornerRadii) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {}
    }

    /// A fixed-size leaf generic over the app state, for a modal's content.
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
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// A frame time `ms` milliseconds in.
    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A pointer event at `(x, y)`.
    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// An Escape key event.
    fn escape() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[derive(Default)]
    struct Flags {
        dismissed: u32,
    }

    fn build(view: &OverlayModalView<Flags>) -> OverlayModalWidget {
        let mut counter = 0u64;
        View::<Flags>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild(
        view: &OverlayModalView<Flags>,
        prev: &OverlayModalView<Flags>,
        w: &mut OverlayModalWidget,
    ) {
        let mut counter = 0u64;
        View::<Flags>::rebuild(view, prev, w, &mut BuildCtx::new(&mut counter));
    }

    fn layout(w: &mut OverlayModalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    fn dispatch(w: &mut OverlayModalWidget, state: &mut Flags, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    fn view(config: OverlayModalConfig) -> OverlayModalView<Flags> {
        overlay_modal(Block(Size::new(200.0, 120.0)), config)
            .on_dismiss(|s: &mut Flags| s.dismissed += 1)
    }

    /// A modal wired the way [`show_overlay_modal`] wires one: a state-free
    /// close hook (here a counter) alongside the unstaged `on_dismiss`, so the
    /// staged exit path is the one taken.
    fn staged(config: OverlayModalConfig) -> (OverlayModalView<Flags>, Rc<Cell<u32>>) {
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let view = view(config).on_close(move || hook.set(hook.get() + 1));
        (view, closed)
    }

    /// Paint one frame at `ms` and return what it drew.
    fn frame(w: &mut OverlayModalWidget, ms: f64) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        rec
    }

    /// Paint (and relay out) until the running ramp is over.
    fn run_ramp(w: &mut OverlayModalWidget, from_ms: f64) {
        frame(w, from_ms);
        layout(w);
        frame(w, from_ms + 2000.0);
        layout(w);
    }

    fn settled(config: OverlayModalConfig) -> OverlayModalConfig {
        config.entrance(OverlayEntrance::None)
    }

    // ---- geometry -----------------------------------------------------------

    #[test]
    fn a_centered_panel_is_capped_centred_and_content_tall() {
        let mut w = build(&view(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        ))));
        let size = layout(&mut w);
        assert_eq!(size, WINDOW, "the host fills its area (the scrim)");
        let panel = w.panel_rect();
        // The viewport margin bites before the 560dp cap in a 400px window.
        assert_eq!(panel.width(), WINDOW.width - 2.0 * CENTERED_MARGIN);
        assert_eq!(panel.height(), 120.0, "content-tall");
        assert!((panel.center().x - WINDOW.width / 2.0).abs() < 1e-9);
        assert!((panel.center().y - WINDOW.height / 2.0).abs() < 1e-9);
        assert_eq!(w.content.origin(), panel.origin());
    }

    #[test]
    fn a_centered_panel_holds_the_m3_minimum_width_where_it_fits() {
        // A cap below the M3 minimum still yields a 280dp panel — the clamp is
        // `[280, max]`, not "whatever the cap says".
        let mut w = build(&view(settled(OverlayModalConfig::centered(120.0))));
        layout(&mut w);
        assert_eq!(w.panel_rect().width(), OVERLAY_DIALOG_MIN_WIDTH);
    }

    #[test]
    fn an_edge_panel_pins_to_its_side_at_three_quarters_capped_at_the_side_sheet_max() {
        for side in [OverlaySide::Left, OverlaySide::Right] {
            let mut w = build(&view(settled(OverlayModalConfig::edge(side))));
            layout(&mut w);
            let panel = w.panel_rect();
            assert_eq!(
                panel.width(),
                (WINDOW.width * OVERLAY_EDGE_FRACTION).min(OVERLAY_SIDE_SHEET_MAX_WIDTH)
            );
            assert_eq!(panel.height(), WINDOW.height, "full height");
            match side {
                OverlaySide::Left => assert_eq!(panel.x0, 0.0),
                _ => assert_eq!(panel.x1, WINDOW.width),
            }
        }
        for side in [OverlaySide::Top, OverlaySide::Bottom] {
            let mut w = build(&view(settled(OverlayModalConfig::edge(side))));
            layout(&mut w);
            let panel = w.panel_rect();
            assert_eq!(panel.width(), WINDOW.width, "full width");
            assert_eq!(panel.height(), 120.0, "content-tall");
            match side {
                OverlaySide::Top => assert_eq!(panel.y0, 0.0),
                _ => assert_eq!(panel.y1, WINDOW.height),
            }
        }
    }

    #[test]
    fn each_edge_preset_rounds_only_the_corners_facing_the_screen() {
        let r = 28.0;
        assert_eq!(
            OverlayModalConfig::edge(OverlaySide::Bottom)
                .corners
                .radii(r),
            CornerRadii::new(r, r, 0.0, 0.0)
        );
        assert_eq!(
            OverlayModalConfig::edge(OverlaySide::Right)
                .corners
                .radii(r),
            CornerRadii::new(r, 0.0, 0.0, r)
        );
        assert_eq!(
            OverlayCorners::None.radii(r),
            CornerRadii::new(0.0, 0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn a_limit_caps_an_edge_panel() {
        let config = settled(OverlayModalConfig::edge(OverlaySide::Bottom).extent(
            OverlayExtent::Content,
            OverlayLimit::Fraction(OVERLAY_SHEET_MAX_HEIGHT_FRACTION),
        ));
        let mut w = build(&overlay_modal(Block(Size::new(200.0, 5000.0)), config));
        layout(&mut w);
        assert_eq!(
            w.panel_rect().height(),
            WINDOW.height * OVERLAY_SHEET_MAX_HEIGHT_FRACTION
        );
    }

    #[test]
    fn an_unbounded_height_collapses_the_host_and_its_panel_to_nothing() {
        // The documented mounts (a navigator page, a full-area `Stack`) are
        // bounded; a host put inside a scroll view is not, and this is what
        // that costs — pinned, not fixed here: the coercion cannot invent an
        // extent nobody offered, so the fix belongs at the mount site.
        let mut w = build(&view(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        ))));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let scrolled = BoxConstraints::new(
            Size::new(WINDOW.width, 0.0),
            Size::new(WINDOW.width, f64::INFINITY),
        );
        let size = w.layout(&mut lctx, &scrolled);
        assert_eq!(size.height, 0.0, "no vertical area to fill");
        assert_eq!(
            w.panel_rect().height(),
            0.0,
            "the panel is a hairline, while the barrier still takes presses"
        );
        // The same host under the contract's own constraints is unharmed.
        let bounded = w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        assert_eq!(bounded, WINDOW);
        assert_eq!(w.panel_rect().height(), 120.0);
    }

    // ---- chrome -------------------------------------------------------------

    #[test]
    fn the_themed_chrome_is_material_roles_end_to_end() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let scheme = theme.scheme();
        let mut w = build(&view(
            settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH))
                .close_button(true)
                .border(OverlayBorder::All),
        ));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);

        assert_eq!(rec.rects[0].1, size, "the scrim covers the area");
        assert_eq!(
            rec.rects[0].2,
            super::super::with_alpha(scheme.scrim, super::super::OVERLAY_SCRIM_ALPHA),
            "the M3 32% scrim"
        );
        assert_eq!(
            rec.panels[0].3, scheme.surface_container_high,
            "the dialog container role"
        );
        assert_eq!(
            rec.panels[0].2,
            CornerRadii::uniform(theme.shape.extra_large),
            "`shape.extra_large` on all four corners"
        );
        let level3 = theme.elevation.level3.shadow(theme.brightness);
        assert_eq!(rec.shadows[0].3, level3.blur_std_dev, "the level-3 rung");
        assert_eq!(rec.strokes[0].2, scheme.outline_variant, "the hairline");
        assert_eq!(
            rec.paths[0].2, scheme.on_surface_variant,
            "the close icon's rest role"
        );
        assert_eq!(rec.clips.len(), 1, "the content is clipped to the panel");
    }

    #[test]
    fn the_unthemed_chrome_falls_back_to_the_m3_baseline_light_values() {
        let mut w = build(&view(
            settled(OverlayModalConfig::edge(OverlaySide::Bottom)).handle(true),
        ));
        layout(&mut w);
        let rec = frame(&mut w, 0.0);
        assert_eq!(rec.rects[0].2, super::super::scrim(None));
        assert_eq!(
            rec.panels[0].3,
            super::super::container(None, OverlayContainer::Low)
        );
        // The M3 32×4dp handle indicator, fully rounded, in the drag-handle
        // color role.
        let handle = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| *s == Size::new(HANDLE_WIDTH, HANDLE_HEIGHT))
            .expect("the drag handle is painted");
        assert_eq!(handle.2, HANDLE_HEIGHT / 2.0);
        assert_eq!(handle.3, super::super::on_surface_variant(None));
        assert!(rec.strokes.is_empty(), "no border by default");
    }

    #[test]
    fn the_close_and_handle_hit_boxes_are_the_m3_touch_target() {
        let mut w = build(&view(
            settled(OverlayModalConfig::edge(OverlaySide::Bottom))
                .close_button(true)
                .handle(true),
        ));
        layout(&mut w);
        let close = w.close_rect().expect("a close box");
        assert_eq!(close.size(), Size::new(CLOSE_HIT, CLOSE_HIT));
        assert_eq!(
            close.center().x,
            w.panel_rect().x1 - CLOSE_INSET - CLOSE_ICON / 2.0
        );
        let handle = w.handle_rect().expect("a handle box");
        assert_eq!(handle.size(), Size::new(CLOSE_HIT, CLOSE_HIT));
        assert_eq!(handle.center().x, w.panel_rect().center().x);
        assert_eq!(OVERLAY_HANDLE_RESERVE, HANDLE_TOUCH_TARGET);
    }

    // ---- dismissal ----------------------------------------------------------

    #[test]
    fn a_scrim_press_and_release_outside_the_panel_dismisses() {
        let mut w = build(&view(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        ))));
        layout(&mut w);
        let mut state = Flags::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0)),
            EventResult::Handled,
            "the barrier swallows the press"
        );
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn a_press_on_the_panel_background_is_swallowed_without_dismissing() {
        let mut w = build(&view(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        ))));
        layout(&mut w);
        let c = w.panel_rect().center();
        let mut state = Flags::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y)),
            EventResult::Handled
        );
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, c.x, c.y));
        assert_eq!(state.dismissed, 0);
    }

    #[test]
    fn scrim_dismiss_off_ignores_the_same_gesture_but_escape_still_works() {
        let mut w = build(&view(
            settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)).scrim_dismiss(false),
        ));
        layout(&mut w);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 0);
        // Escape still works — the alert dialog's own escape hatch.
        dispatch(&mut w, &mut state, &escape());
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn dismissable_false_gates_every_gesture_at_once() {
        let mut w = build(
            &view(
                settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)).close_button(true),
            )
            .dismissable(false),
        );
        layout(&mut w);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        dispatch(&mut w, &mut state, &escape());
        let close = w.close_rect().expect("a close box").center();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, close.x, close.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, close.x, close.y),
        );
        assert_eq!(
            state.dismissed, 0,
            "the scrim tap, Escape and the close affordance are all gated together"
        );
    }

    #[test]
    fn the_close_affordance_fires_on_release_inside_and_hovers() {
        let mut w = build(&view(
            settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)).close_button(true),
        ));
        layout(&mut w);
        let close = w.close_rect().expect("a close box");
        let c = close.center();
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        assert!(w.close_hovered, "the latch follows the hit test");
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        assert_eq!(state.dismissed, 0, "never on down");
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, c.x, c.y));
        assert_eq!(state.dismissed, 1);

        // A release outside the box fires nothing, and the press is not a scrim
        // press either.
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn a_hovered_close_affordance_steps_up_to_the_on_surface_role() {
        // A real `RenderRoot` is the only harness that can exercise hover at
        // all: the link is recorded by the root's event pass and read back
        // through `PaintCtx::is_hovered`, which the paint arm treats as
        // authoritative (a bare paint reads `false` and clears the latch, by
        // design). The `crate::list_item` hover-harness precedent.
        let config =
            settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)).close_button(true);
        // The geometry is deterministic, so a throwaway widget answers where
        // the close box lands.
        let mut probe = build(&view(config));
        layout(&mut probe);
        let c = probe.close_rect().expect("a close box").center();

        let mut root: RenderRoot<Flags, OverlayModalView<Flags>> = RenderRoot::new();
        let mut state = Flags::default();
        let mut tcx = TextContext::new();
        let mut app = move |_s: &mut Flags| overlay_modal(Block(Size::new(200.0, 120.0)), config);
        let theme = crate::baseline().with_brightness(Brightness::Light);
        root.set_theme(Box::new(theme.clone()));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        let mut rest = Recorder::default();
        root.paint(&mut rest, FrameTime::ZERO);
        assert_eq!(
            rest.paths[0].2,
            theme.scheme().on_surface_variant,
            "at rest the affordance sits in the muted ink role"
        );

        root.event(&mut state, &pointer(PointerPhase::Move, c.x, c.y));
        let mut hovered = Recorder::default();
        root.paint(&mut hovered, FrameTime::ZERO);
        assert_eq!(
            hovered.paths[0].2,
            theme.scheme().on_surface,
            "a hovered affordance steps up to the panel's own ink"
        );
    }

    #[test]
    fn the_drag_handle_dismisses_on_release() {
        let mut w = build(&view(
            settled(OverlayModalConfig::edge(OverlaySide::Bottom)).handle(true),
        ));
        layout(&mut w);
        let handle = w.handle_rect().expect("a handle box");
        let c = handle.center();
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, c.x, c.y));
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn a_cancelled_press_clears_every_latch_without_dismissing() {
        let mut w = build(&view(
            settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)).close_button(true),
        ));
        layout(&mut w);
        let c = w.close_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Cancel, c.x, c.y));
        assert_eq!(state.dismissed, 0);
        assert!(!w.close_captured && !w.scrim_captured);
    }

    #[test]
    fn the_barrier_swallows_a_key_the_content_did_not_take() {
        let mut w = build(&view(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        ))));
        layout(&mut w);
        let mut state = Flags::default();
        let tab = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &tab),
            EventResult::Handled,
            "a modal barrier absorbs keys as well as pointers"
        );
        assert_eq!(state.dismissed, 0, "and only Escape dismisses");
    }

    // ---- motion -------------------------------------------------------------

    #[test]
    fn the_fade_scale_entrance_ramps_alpha_and_scale_then_settles() {
        let mut w = build(&view(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        )));
        layout(&mut w);
        let first = frame(&mut w, 0.0);
        assert_eq!(first.layers, vec![0.0], "starts transparent");
        assert_eq!(first.rects[0].2.components[3], 0.0, "so does the scrim");
        let mid = frame(&mut w, MaterialMotion::SHORT_4.as_millis() as f64 / 2.0);
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "mid-fade: {alpha}");
        assert_eq!(mid.transforms.len(), 1, "and a scale transform");
        let done = frame(&mut w, 1000.0);
        assert!(
            done.layers.is_empty() && done.transforms.is_empty(),
            "a settled panel composites plainly"
        );
        assert!((w.progress() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_entrance_durations_are_the_mapped_material_presets() {
        assert_eq!(
            OverlayEntrance::FadeScale.duration(),
            MaterialMotion::SHORT_4
        );
        assert_eq!(OverlayEntrance::Slide.duration(), MaterialMotion::LONG_2);
        assert_eq!(OverlayEntrance::None.duration(), Duration::ZERO);
        assert_eq!(
            OverlayEntrance::FadeScale.curve(false),
            MaterialMotion::EMPHASIZED_DECELERATE
        );
        assert_eq!(
            OverlayEntrance::Slide.curve(false),
            MaterialMotion::EMPHASIZED
        );
        assert_eq!(
            OverlayEntrance::Slide.curve(true),
            MaterialMotion::EMPHASIZED_ACCELERATE,
            "either entrance leaves on M3's accelerate curve"
        );
    }

    #[test]
    fn reduce_motion_collapses_the_entrance_to_a_jump() {
        let mut theme = crate::baseline().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;
        let mut w = build(&view(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        )));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert!((w.progress() - 1.0).abs() < 1e-9, "settled on the spot");
        assert!(rec.layers.is_empty(), "no fade layer");
        assert_eq!(
            rec.rects[0].2,
            super::super::with_alpha(theme.scheme().scrim, super::super::OVERLAY_SCRIM_ALPHA),
            "a full-strength scrim"
        );
    }

    #[test]
    fn the_slide_entrance_moves_the_panel_in_layout() {
        let mut w = build(&view(OverlayModalConfig::edge(OverlaySide::Right)));
        layout(&mut w);
        assert_eq!(
            w.panel_rect().x0,
            WINDOW.width,
            "starts entirely off the right edge"
        );
        let step = |w: &mut OverlayModalWidget, ms: f64| {
            frame(w, ms);
            layout(w);
        };
        step(&mut w, 0.0);
        step(&mut w, MaterialMotion::LONG_2.as_millis() as f64 / 2.0);
        let mid = w.panel_rect();
        assert!(
            mid.x0 > WINDOW.width - mid.width() && mid.x0 < WINDOW.width,
            "mid-slide: {}",
            mid.x0
        );
        step(&mut w, 2000.0);
        assert_eq!(w.panel_rect().x1, WINDOW.width, "flush at rest");
    }

    #[test]
    fn each_entrances_seeding_paint_asks_for_the_pass_its_own_motion_needs() {
        // `AnimationController::advance`'s first truthy call after `forward()`
        // only seeds the clock — `progress` is still 0.0 after it. The
        // continuation has to be requested on that seeding paint too, or a
        // layout-affecting entrance never schedules the frame that would carry
        // it off zero, and the panel sits permanently off-screen under a
        // `ControlFlow::Wait` shell.
        for (config, layout_affecting) in [
            (OverlayModalConfig::edge(OverlaySide::Right), true),
            (
                OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
                false,
            ),
        ] {
            let mut w = build(&view(config));
            let size = layout(&mut w);
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, ft_ms(0.0));
            w.paint(&mut ctx, &mut Recorder::default());
            assert!(
                w.progress().abs() < 1e-9,
                "the seeding paint leaves progress at zero"
            );
            assert!(
                ctx.needs_frame(),
                "the seeding paint must schedule the frame that ramps progress off zero"
            );
            assert_eq!(
                ctx.needs_layout(),
                layout_affecting,
                "only a layout-affecting entrance asks for a layout pass"
            );
        }
    }

    // ---- the staged exit ----------------------------------------------------

    #[test]
    fn a_dismiss_ramps_the_panel_out_and_closes_only_when_it_settles() {
        let (view, closed) = staged(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH));
        let mut w = build(&view);
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        assert!((w.progress() - 1.0).abs() < 1e-9, "entered");

        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 0, "the unstaged callback stays unused");
        assert_eq!(closed.get(), 0, "and the close is deferred");
        assert!(w.is_exiting());

        // Mid-ramp: still visible, still nothing fired.
        frame(&mut w, 3000.0);
        let mid = frame(
            &mut w,
            3000.0 + MaterialMotion::SHORT_4.as_millis() as f64 / 2.0,
        );
        assert!(w.progress() > 0.0 && w.progress() < 1.0, "{}", w.progress());
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "the panel fades out: {alpha}");
        assert!(
            mid.rects[0].2.components[3] < super::super::OVERLAY_SCRIM_ALPHA,
            "and the scrim rides the ramp down"
        );
        assert_eq!(closed.get(), 0);

        // Settled: progress at zero, and the close fires exactly once.
        frame(&mut w, 5000.0);
        assert_eq!(w.progress(), 0.0);
        assert_eq!(closed.get(), 1);
        assert!(!w.is_exiting());
        assert_eq!(state.dismissed, 0);
    }

    #[test]
    fn the_staged_close_hook_survives_a_rebuild_and_still_fires_exactly_once() {
        // `OverlayModalView::rebuild` never writes `element.on_close` at all
        // (see its doc comment); `StagedExit::rebuild` — which runs
        // `inner.rebuild` first and reinstalls its own hook after — must still
        // be the writer that wins on the staged (`show_overlay_modal`) path,
        // through as many rebuilds as the page sees before it's popped.
        let closed = Rc::new(Cell::new(0u32));
        let config = OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH);
        let hook1 = closed.clone();
        let v1 = StagedExit {
            inner: view(config),
            on_close: Rc::new(move || hook1.set(hook1.get() + 1)) as OnClose,
            dismiss_signal: None,
        };
        let mut counter = 0u64;
        let mut w = View::<Flags>::build(&v1, &mut BuildCtx::new(&mut counter));
        layout(&mut w);
        run_ramp(&mut w, 0.0);

        // A second rebuild pass through the wrapper, as the navigator's pushed
        // page sees on every frame it stays mounted.
        let hook2 = closed.clone();
        let v2 = StagedExit {
            inner: view(config),
            on_close: Rc::new(move || hook2.set(hook2.get() + 1)) as OnClose,
            dismiss_signal: None,
        };
        View::<Flags>::rebuild(&v2, &v1, &mut w, &mut BuildCtx::new(&mut counter));

        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        frame(&mut w, 3000.0);
        frame(&mut w, 5000.0);
        assert_eq!(
            closed.get(),
            1,
            "the reinstalled hook still fires exactly once"
        );
    }

    #[test]
    fn a_rebuild_never_resurrects_or_clears_a_bare_views_on_close_hook() {
        // Pinned behavior, not last-writer-wins: a directly constructed,
        // non-staged view's `on_close` is installed once at `build` and never
        // touched by a later `rebuild`. A rebuild carrying `None` must not clear
        // the hook the widget already has installed.
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let v1 = view(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH))
            .on_close(move || hook.set(hook.get() + 1));
        let mut w = build(&v1);
        layout(&mut w);
        run_ramp(&mut w, 0.0);

        // Rebuild with a view whose own `on_close` is unset.
        let v2 = view(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH));
        rebuild(&v2, &v1, &mut w);

        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        frame(&mut w, 3000.0);
        frame(&mut w, 5000.0);
        assert_eq!(
            closed.get(),
            1,
            "the build-time hook is still installed and still fires — pinned, not cleared"
        );
    }

    #[test]
    fn the_exit_keeps_asking_for_the_pass_its_own_motion_needs() {
        for (config, layout_affecting) in [
            (OverlayModalConfig::edge(OverlaySide::Bottom), true),
            (
                OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
                false,
            ),
        ] {
            let (view, _closed) = staged(config);
            let mut w = build(&view);
            layout(&mut w);
            run_ramp(&mut w, 0.0);
            let mut state = Flags::default();
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, 5.0));
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 5.0, 5.0));

            let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(3000.0));
            w.paint(&mut ctx, &mut Recorder::default());
            assert!(
                ctx.needs_frame(),
                "the exit's seeding paint schedules its own continuation"
            );
            assert_eq!(
                ctx.needs_layout(),
                layout_affecting,
                "only a layout-affecting exit asks for a layout pass"
            );
        }
    }

    #[test]
    fn reduce_motion_closes_on_the_spot_with_no_ramp() {
        let mut theme = crate::baseline().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;
        let (view, closed) = staged(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH));
        let mut w = build(&view);
        let size = layout(&mut w);
        let paint = |w: &mut OverlayModalWidget, ms: f64| {
            let mut ctx =
                PaintCtx::for_test(Point::ORIGIN, size, ft_ms(ms)).with_theme(&theme as &dyn Any);
            w.paint(&mut ctx, &mut Recorder::default());
        };
        paint(&mut w, 0.0);
        assert!((w.progress() - 1.0).abs() < 1e-9, "no entrance ramp");

        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        paint(&mut w, 16.0);
        assert_eq!(w.progress(), 0.0, "the exit is a jump, not a ramp");
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn a_second_dismiss_while_exiting_is_a_no_op() {
        let (view, closed) = staged(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH));
        let mut w = build(&view);
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        let first = w.progress();
        frame(&mut w, 3000.0);
        frame(
            &mut w,
            3000.0 + MaterialMotion::SHORT_4.as_millis() as f64 / 2.0,
        );
        let mid = w.progress();
        assert!(mid < first, "the ramp is running");

        // A second trigger neither restarts the ramp nor fires anything.
        dispatch(&mut w, &mut state, &escape());
        assert_eq!(w.progress(), mid, "the ramp is untouched");
        frame(&mut w, 5000.0);
        assert_eq!(closed.get(), 1, "one close for two triggers");
        assert_eq!(state.dismissed, 0);
    }

    #[test]
    fn a_dismiss_trigger_after_settle_does_not_fire_on_close_again() {
        // The hazard: `on_close` only *enqueues* `controller.pop()`, so the
        // widget stays mounted — and still a barrier — for at least one frame
        // after the exit ramp settles. A trigger landing in that window would
        // otherwise re-enter `request_dismiss` with `progress` already at zero:
        // a zero-duration ramp settling on its very next paint, firing
        // `on_close` a second time and popping the page underneath this modal.
        let (view, closed) = staged(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH));
        let mut w = build(&view);
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        frame(&mut w, 3000.0);
        frame(&mut w, 5000.0);
        assert_eq!(w.progress(), 0.0, "settled closed");
        assert_eq!(closed.get(), 1, "on_close fired once on settle");
        assert!(!w.is_exiting());

        // The enqueued pop hasn't drained yet, so the widget is still mounted. A
        // further dismiss trigger must be a pure no-op.
        dispatch(&mut w, &mut state, &escape());
        frame(&mut w, 7000.0);
        assert_eq!(closed.get(), 1, "on_close does not fire a second time");
        assert_eq!(state.dismissed, 0);
    }

    #[test]
    fn without_a_close_hook_the_dismissal_stays_unstaged() {
        // A `Stack`-mounted modal that only wired `on_dismiss` has no
        // state-bearing pass to defer into, so it keeps the immediate path
        // rather than leaving an invisible barrier standing.
        let mut w = build(&view(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        )));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        assert_eq!(state.dismissed, 1);
        assert!(!w.is_exiting());
        assert!((w.progress() - 1.0).abs() < 1e-9, "nothing animates out");
    }

    // ---- the back-press (`DismissAnimated`) tier ----------------------------

    /// A staged modal wired the way [`show_overlay_modal`] wires one, plus the
    /// dismiss-signal cell the navigator bumps on a back press.
    fn back_wired(
        config: OverlayModalConfig,
    ) -> (OverlayModalWidget, Rc<Cell<u64>>, Rc<Cell<u32>>) {
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let signal = Rc::new(Cell::new(0u64));
        let staged = StagedExit {
            inner: view(config),
            on_close: Rc::new(move || hook.set(hook.get() + 1)) as OnClose,
            dismiss_signal: Some(signal.clone()),
        };
        let mut counter = 0u64;
        let mut w = View::<Flags>::build(&staged, &mut BuildCtx::new(&mut counter));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        (w, signal, closed)
    }

    #[test]
    fn a_dismiss_signal_bump_stages_the_same_exit_ramp_every_gesture_takes() {
        let (mut w, signal, closed) =
            back_wired(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH));
        assert!((w.progress() - 1.0).abs() < 1e-9, "entered");

        // The navigator's back press: bump the shared generation cell.
        signal.set(signal.get() + 1);
        frame(&mut w, 3000.0);
        assert!(w.is_exiting(), "the back press stages the exit, not a pop");
        assert_eq!(closed.get(), 0, "and fires nothing yet");
        frame(
            &mut w,
            3000.0 + MaterialMotion::SHORT_4.as_millis() as f64 / 2.0,
        );
        assert!(w.progress() > 0.0 && w.progress() < 1.0, "mid-exit");
        frame(&mut w, 5000.0);
        assert_eq!(closed.get(), 1, "the pop fires when the ramp settles");

        // A stale re-observation of the same generation changes nothing.
        frame(&mut w, 6000.0);
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn a_back_press_on_an_unstageable_modal_fires_the_pop_and_schedules_its_drain() {
        // No entrance to reverse: the close fires on the spot, and the very
        // paint that fires it must request the frame whose rebuild drains the
        // enqueued pop (the drain gotcha).
        let (mut w, signal, closed) = back_wired(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        )));
        signal.set(signal.get() + 1);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(3000.0));
        w.paint(&mut ctx, &mut Recorder::default());
        assert_eq!(closed.get(), 1);
        assert!(
            ctx.needs_frame(),
            "the firing paint schedules the draining frame"
        );
    }

    #[test]
    fn a_non_dismissable_modal_ignores_a_dismiss_signal_bump() {
        // `show_overlay_modal` pushes such a page with `BackPolicy::Veto` and no
        // cell at all; the widget-side guard is belt-and-braces for a modal
        // whose flag flips after the push.
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let signal = Rc::new(Cell::new(0u64));
        let staged = StagedExit {
            inner: view(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)).dismissable(false),
            on_close: Rc::new(move || hook.set(hook.get() + 1)) as OnClose,
            dismiss_signal: Some(signal.clone()),
        };
        let mut counter = 0u64;
        let mut w = View::<Flags>::build(&staged, &mut BuildCtx::new(&mut counter));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        signal.set(signal.get() + 1);
        frame(&mut w, 3000.0);
        assert!(!w.is_exiting());
        frame(&mut w, 5000.0);
        assert_eq!(closed.get(), 0, "a Veto-shaped modal never self-pops");
    }

    #[test]
    fn a_bump_that_predates_the_widget_is_not_a_back_press_aimed_at_it() {
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let signal = Rc::new(Cell::new(7u64));
        let staged = StagedExit {
            inner: view(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH)),
            on_close: Rc::new(move || hook.set(hook.get() + 1)) as OnClose,
            dismiss_signal: Some(signal.clone()),
        };
        let mut counter = 0u64;
        let mut w = View::<Flags>::build(&staged, &mut BuildCtx::new(&mut counter));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        assert!(
            !w.is_exiting(),
            "the pre-existing generation is the baseline"
        );
        assert_eq!(closed.get(), 0);
    }

    // ---- navigator integration ---------------------------------------------

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<i32>>,
    }

    /// A minimal opaque page for the navigator integration tests.
    struct BgPage(Size);

    /// The retained half of [`BgPage`].
    struct BgPageWidget(Size);

    impl View<NavState> for BgPage {
        type Element = BgPageWidget;
        fn build(&self, _c: &mut BuildCtx<'_>) -> BgPageWidget {
            BgPageWidget(self.0)
        }
        fn rebuild(&self, _p: &Self, _e: &mut BgPageWidget, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for BgPageWidget {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    /// A navigator rooted on an opaque page, plus the controller driving it.
    struct NavHarness {
        controller: NavigatorController<NavState>,
        root: RenderRoot<NavState, NavigatorView<NavState>>,
        state: NavState,
        tcx: TextContext,
    }

    impl NavHarness {
        fn new() -> Self {
            let controller: NavigatorController<NavState> = NavigatorController::new();
            let mut h = NavHarness {
                controller,
                root: RenderRoot::new(),
                state: NavState::default(),
                tcx: TextContext::new(),
            };
            h.pass();
            h
        }

        fn app(&self) -> impl FnMut(&mut NavState) -> NavigatorView<NavState> + use<> {
            let ctrl = self.controller.clone();
            move |_: &mut NavState| navigator(&ctrl, || core_any::<NavState, _>(BgPage(WINDOW)))
        }

        fn pass(&mut self) {
            let mut app = self.app();
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self, ms: u64) -> bool {
            self.root
                .paint(&mut Recorder::default(), ft_ms(ms as f64))
                .needs_frame
        }

        /// Drive rebuild → layout → paint until nothing asks for another frame
        /// (bounded), the shape a real shell's loop takes.
        fn drive(&mut self, from_ms: u64) {
            for i in 0..20u64 {
                self.pass();
                if !self.paint(from_ms + i * 100) {
                    return;
                }
            }
            panic!("the frame loop never settled");
        }
    }

    #[test]
    fn show_overlay_modal_pushes_a_transparent_page_that_pops_on_a_staged_dismiss() {
        let mut h = NavHarness::new();
        show_overlay_modal(
            &h.controller,
            || {
                overlay_modal(
                    Block(Size::new(200.0, 120.0)),
                    OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
                )
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        h.pass();
        h.drive(0);
        assert_eq!(h.controller.depth(), 2, "the modal is up");

        // A scrim tap stages the exit; the pop lands once it settles.
        h.root
            .event(&mut h.state, &pointer(PointerPhase::Down, 5.0, 5.0));
        h.root
            .event(&mut h.state, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(
            h.controller.depth(),
            2,
            "nothing pops during the exit ramp itself"
        );
        h.drive(3000);
        assert_eq!(h.state.results, vec![None], "popped with an empty result");
        assert_eq!(h.controller.depth(), 1);
    }

    #[test]
    fn a_back_request_dismisses_a_dismissable_modal_through_the_navigator() {
        let mut h = NavHarness::new();
        show_overlay_modal(
            &h.controller,
            || {
                overlay_modal(
                    Block(Size::new(200.0, 120.0)),
                    OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
                )
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        h.pass();
        h.drive(0);
        assert!(
            h.controller.back_interest(),
            "a dismissable modal claims back interest"
        );

        h.controller.request_back();
        h.pass();
        assert_eq!(
            h.controller.depth(),
            2,
            "DismissAnimated: the stack is unchanged immediately"
        );
        // The back-press rebuild flagged PAINT; this paint observes the bumped
        // signal and stages the exit — which pops only once it settles.
        assert!(
            h.paint(3000),
            "the staging paint schedules its own continuation"
        );
        assert_eq!(h.controller.depth(), 2, "still up, mid-ramp");
        h.drive(4000);
        assert_eq!(
            h.state.results,
            vec![None],
            "a back request dismisses the modal"
        );
        assert_eq!(h.controller.depth(), 1);
    }

    #[test]
    fn a_back_request_on_a_non_dismissable_modal_is_vetoed() {
        let mut h = NavHarness::new();
        show_overlay_modal(
            &h.controller,
            || {
                overlay_modal(
                    Block(Size::new(200.0, 120.0)),
                    OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
                )
                .dismissable(false)
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        h.pass();
        h.drive(0);
        assert!(
            h.controller.back_interest(),
            "a Veto modal still claims the back press"
        );

        h.controller.request_back();
        h.pass();
        h.paint(3000);
        h.paint(6000);
        assert!(
            h.state.results.is_empty(),
            "a non-dismissable modal never pops on a back request"
        );
        assert_eq!(h.controller.depth(), 2);
    }

    // ---- drag-to-close and snap points --------------------------------------

    /// A bottom sheet's chrome: content-tall, draggable, handled.
    fn sheet_config() -> OverlayModalConfig {
        OverlayModalConfig::edge(OverlaySide::Bottom)
            .handle(true)
            .drag(true)
    }

    /// Build an entered, draggable bottom panel plus its close counter.
    fn dragged() -> (OverlayModalWidget, Rc<Cell<u32>>) {
        let (view, closed) = staged(sheet_config());
        let mut w = build(&view);
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        (w, closed)
    }

    #[test]
    fn a_drag_past_the_midpoint_closes_and_a_shorter_one_springs_back() {
        for (travel, closes) in [(100.0, true), (30.0, false)] {
            let (mut w, closed) = dragged();
            let extent = w.panel_rect().height();
            let start = w.handle_rect().unwrap().center();
            let mut state = Flags::default();
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Down, start.x, start.y),
            );
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Move, start.x, start.y + travel),
            );
            // The panel tracks the pointer: progress falls by the fraction of
            // its own extent the drag covered.
            let expected = 1.0 - travel / extent;
            assert!(
                (w.progress() - expected).abs() < 1e-9,
                "{travel} → {}",
                w.progress()
            );
            assert!(
                w.panel_rect().y0 > 0.0 && w.panel_rect().y1 > WINDOW.height,
                "the panel followed it down"
            );
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Up, start.x, start.y + travel),
            );
            run_ramp(&mut w, 3000.0);
            if closes {
                assert_eq!(closed.get(), 1, "past the midpoint it commits");
                assert_eq!(w.progress(), 0.0);
            } else {
                assert_eq!(closed.get(), 0, "short of it, it settles back open");
                assert!((w.progress() - 1.0).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn a_non_dismissable_sheet_springs_back_from_a_drag_past_the_midpoint() {
        // Neither `on_dismiss` nor `on_close` wired: `overlay_modal` alone, not
        // the `view`/`staged` helpers above (both wire `on_dismiss`) — the shape
        // a `Stack`-mounted sheet with no dismiss hook is left in.
        let mut w = build(&overlay_modal(
            Block(Size::new(200.0, 120.0)),
            sheet_config(),
        ));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let extent = w.panel_rect().height();
        let start = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + extent * 0.9),
        );
        assert!(
            w.progress() < 0.2,
            "the panel still tracks the pointer while dragging: {}",
            w.progress()
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, start.x, start.y + extent * 0.9),
        );
        run_ramp(&mut w, 3000.0);
        assert!(
            (w.progress() - 1.0).abs() < 1e-9,
            "no dismiss hook: springs back to open_progress() instead of \
             sticking near zero as an invisible full-window barrier — {}",
            w.progress()
        );
        // The panel is unambiguously open and hittable where it rests, not a
        // residual off-screen sliver.
        assert!(w.panel_rect().contains(w.panel_rect().center()));
        assert!(w.panel_rect().y0 < WINDOW.height && w.panel_rect().y1 == WINDOW.height);
        assert_eq!(state.dismissed, 0, "no dismiss hook ever fired");
    }

    #[test]
    fn snap_points_zero_is_treated_as_absent_and_opens_fully() {
        let (view, _closed) = staged(sheet_config());
        let mut w = build(&view.snap_points(&[0.0]));
        assert!(
            w.open_progress() > PROGRESS_EPSILON,
            "a first point of 0.0 is treated as absent: {}",
            w.open_progress()
        );
        assert_eq!(w.open_progress(), 1.0, "falls back to fully open");

        layout(&mut w);
        run_ramp(&mut w, 0.0);
        assert!(
            (w.progress() - 1.0).abs() < 1e-9,
            "the sheet opens fully rather than opening already closed: {}",
            w.progress()
        );
    }

    #[test]
    fn a_flick_toward_the_edge_closes_from_anywhere() {
        // A tall panel, so the flick's own travel stays a small fraction of the
        // extent and the *speed* is what decides.
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let mut w = build(
            &overlay_modal(Block(Size::new(200.0, 400.0)), sheet_config())
                .on_close(move || hook.set(hook.get() + 1)),
        );
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let start = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        // A short, fast flick: 40px between two frames 16ms apart is 2500 px/s,
        // well past `OVERLAY_FLING_VELOCITY`, while the panel is still nearly
        // open.
        frame(&mut w, 3000.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + 40.0),
        );
        frame(&mut w, 3016.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + 80.0),
        );
        assert!(w.progress() > 0.5, "still mostly open: {}", w.progress());
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, start.x, start.y + 80.0),
        );
        run_ramp(&mut w, 4000.0);
        assert_eq!(closed.get(), 1, "the flick committed to the close");
    }

    #[test]
    fn catching_a_closing_sheet_cancels_its_dismissal() {
        let (mut w, closed) = dragged();
        let mut state = Flags::default();
        let handle = w.handle_rect().unwrap().center();
        dispatch(&mut w, &mut state, &escape());
        frame(&mut w, 3000.0);
        frame(
            &mut w,
            3000.0 + MaterialMotion::LONG_2.as_millis() as f64 / 4.0,
        );
        assert!(w.is_exiting() && w.progress() < 1.0);

        // Grabbing the panel mid-exit and pulling it back open wins.
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, handle.x, handle.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, handle.x, handle.y - 60.0),
        );
        assert!(!w.is_exiting(), "the drag owns the panel now");
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, handle.x, handle.y - 60.0),
        );
        run_ramp(&mut w, 4000.0);
        assert_eq!(closed.get(), 0, "nothing closed");
        assert!((w.progress() - 1.0).abs() < 1e-9, "back open");
    }

    #[test]
    fn a_cancelled_drag_returns_to_where_it_started() {
        let (mut w, closed) = dragged();
        let start = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + 90.0),
        );
        assert!(w.progress() < 1.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Cancel, start.x, start.y + 90.0),
        );
        run_ramp(&mut w, 3000.0);
        assert!((w.progress() - 1.0).abs() < 1e-9, "restored");
        assert_eq!(closed.get(), 0);
    }

    #[test]
    fn a_tap_on_a_draggable_handle_still_dismisses_but_one_on_the_panel_does_not() {
        let (mut w, closed) = dragged();
        let mut state = Flags::default();
        let handle = w.handle_rect().unwrap().center();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, handle.x, handle.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, handle.x, handle.y),
        );
        run_ramp(&mut w, 3000.0);
        assert_eq!(closed.get(), 1);

        let (mut w, closed) = dragged();
        let body = w.panel_rect().center();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, body.x, body.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, body.x, body.y),
        );
        run_ramp(&mut w, 3000.0);
        assert_eq!(
            closed.get(),
            0,
            "the panel surface is a barrier, not a button"
        );
        assert!((w.progress() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn snap_points_open_at_the_first_and_the_release_takes_the_nearest() {
        let (view, closed) = staged(sheet_config());
        let mut w = build(&view.snap_points(&[0.5, 1.0]));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        assert!(
            (w.progress() - 0.5).abs() < 1e-9,
            "opens at the first point: {}",
            w.progress()
        );
        let extent = w.panel_rect().height();
        assert!(
            (w.panel_rect().y0 - (WINDOW.height - extent * 0.5)).abs() < 1e-9,
            "and shows exactly that fraction of itself"
        );

        // Dragged up most of the way: the release snaps to the point above.
        let start = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y - extent * 0.4),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, start.x, start.y - extent * 0.4),
        );
        run_ramp(&mut w, 3000.0);
        assert!((w.progress() - 1.0).abs() < 1e-9, "{}", w.progress());
        assert_eq!(closed.get(), 0);

        // Below the lowest point, `0.0` is the nearest: it closes.
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + extent * 0.85),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, start.x, start.y + extent * 0.85),
        );
        run_ramp(&mut w, 6000.0);
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn a_snap_point_above_one_is_logical_px_against_the_extent() {
        let (view, _closed) = staged(sheet_config());
        let mut w = build(&view.snap_points(&[60.0]));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let extent = w.panel_rect().height();
        assert!(
            (w.progress() - 60.0 / extent).abs() < 1e-9,
            "{}",
            w.progress()
        );
        assert!(
            (w.panel_rect().y0 - (WINDOW.height - 60.0)).abs() < 1e-9,
            "60 logical px of the sheet are on screen"
        );
    }

    #[test]
    fn snap_points_drops_non_finite_values_at_the_boundary() {
        let (view, _closed) = staged(sheet_config());
        let view = view.snap_points(&[f64::NAN, f64::INFINITY]);
        assert!(
            view.snap_points.is_empty(),
            "NaN and infinity are both non-finite: dropped by the builder, \
             never stored"
        );
    }

    #[test]
    fn non_finite_snap_points_behave_as_absent_through_a_drag_and_never_panic() {
        // Same shape as `a_drag_past_the_midpoint_closes_and_a_shorter_one_
        // springs_back`, but with degenerate snap points: NaN/∞ are dropped
        // at the boundary (`OverlayModalView::snap_points`), so
        // `snap_targets()` is exactly the no-snap-points default `{0.0, 1.0}`
        // and the drag clamp's `open` bound is never NaN — the crash this
        // guards against is `f64::clamp`'s internal `assert!(min <= max)`
        // panicking on a NaN bound, an abort under `panic = "abort"`.
        let (view, closed) = staged(sheet_config());
        let mut w = build(&view.snap_points(&[f64::NAN, f64::INFINITY]));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        assert!(
            (w.progress() - 1.0).abs() < 1e-9,
            "opens fully, same as no snap points at all: {}",
            w.progress()
        );

        let extent = w.panel_rect().height();
        let start = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, start.x, start.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Move, start.x, start.y + extent * 0.9),
        );
        assert!(
            w.progress().is_finite(),
            "no NaN reached progress mid-drag: {}",
            w.progress()
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, start.x, start.y + extent * 0.9),
        );
        run_ramp(&mut w, 3000.0);
        assert!(w.progress().is_finite());
        assert_eq!(
            closed.get(),
            1,
            "past the midpoint it commits, same as the no-snap-points default"
        );
    }

    #[test]
    fn degenerate_snap_points_dont_strand_the_barrier_on_a_tap_dismiss() {
        // A handle tap (not a drag) still routes through the same exit-ramp
        // settle check (`settled_exit`'s `progress <= PROGRESS_EPSILON`).
        // Degenerate snap points must not leave a NaN in `progress` that
        // would never satisfy that comparison and strand the barrier open
        // forever.
        let (view, closed) = staged(sheet_config());
        let mut w = build(&view.snap_points(&[f64::NAN, f64::INFINITY]));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let handle = w.handle_rect().unwrap().center();
        let mut state = Flags::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, handle.x, handle.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, handle.x, handle.y),
        );
        run_ramp(&mut w, 3000.0);
        assert!(
            w.progress().is_finite(),
            "no NaN reached progress: {}",
            w.progress()
        );
        assert_eq!(w.progress(), 0.0, "settled fully closed, not stranded");
        assert_eq!(
            closed.get(),
            1,
            "the barrier settled and fired on_close, not left stuck open"
        );
    }

    // ---- semantics ----------------------------------------------------------

    #[test]
    fn semantics_is_a_modal_node_labelled_by_its_title() {
        fn logic(_s: &mut Flags) -> OverlayModalView<Flags> {
            overlay_modal(
                Block(Size::new(100.0, 50.0)),
                settled(OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH))
                    .role(OverlayRole::AlertDialog),
            )
            .label("Are you sure?")
        }
        let mut root: RenderRoot<Flags, OverlayModalView<Flags>> = RenderRoot::new();
        let mut state = Flags::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::AlertDialog)
            .expect("an AlertDialog node");
        assert!(node.is_modal());
        assert_eq!(node.label(), Some("Are you sure?"));
    }

    #[test]
    fn visit_children_publishes_the_content() {
        let w = build(&view(settled(OverlayModalConfig::centered(
            OVERLAY_DIALOG_MAX_WIDTH,
        ))));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }
}
