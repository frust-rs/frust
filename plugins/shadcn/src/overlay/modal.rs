//! The modal overlay host: shadcn's `bg-black/50` scrim plus one panel, as a
//! single widget the five modal components (dialog, alert-dialog, sheet,
//! drawer, command dialog) configure rather than re-derive.
//!
//! Sources, all shadcn/ui v4 rev `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`
//! (retrieved 2026-08-17): `dialog.tsx`, `alert-dialog.tsx`, `sheet.tsx`,
//! `drawer.tsx`, `command.tsx`. Upstream these are five Radix/vaul wrappers over
//! the *same* two elements — an `Overlay` (`fixed inset-0 z-50 bg-black/50`) and
//! a `Content` (`fixed z-50 … bg-background`) — differing only in where the
//! content is pinned, which corners it rounds, which edge it borders, and how it
//! animates in. [`ModalConfig`] is exactly that difference list, so a component
//! module contributes its chrome config plus the composed content view and
//! nothing else.
//!
//! # Architecture
//!
//! Mirrors `frust_material::dialog` end to end (see [`super`]'s mounting note):
//! the scrim belongs to *this* widget rather than to the navigator, the panel is
//! a modal barrier that swallows every pointer event its content did not take,
//! dismissal is the app's `on_dismiss` (wired to `controller.pop()` by
//! [`show_modal`]), and an action inside the content pops with a value through
//! the navigator's own pop-result machinery. Only the chrome is shadcn's.
//!
//! Three dismiss gestures, each independently disableable:
//!
//! * a **scrim tap** — a press *and* release outside the panel, when
//!   [`ModalConfig::scrim_dismiss`] is on (alert-dialog turns it off, per its
//!   source: an alert dialog demands an explicit choice);
//! * the **close button** — the `absolute top-4 right-4` X, when
//!   [`ModalConfig::close_button`] is on;
//! * the **drag handle** — the drawer's `h-2 w-[100px]` bar, when
//!   [`ModalConfig::handle`] is on. A press and release on it dismisses; with
//!   [`ModalConfig::drag`] on it is also the drag affordance (below).
//!
//! plus **Escape**, once the modal holds focus — claimed on every `Down`, the
//! `frust_material::dialog` opt-in, with the same documented gap: there is no
//! auto-focus-on-appear hook in the framework, so a caller must complete one
//! pointer interaction with the modal before Escape does anything.
//!
//! # The barrier swallows keys too
//!
//! The panel is a modal barrier for *every* input class, not just pointers: a
//! key the content did not take is reported [`EventResult::Handled`] rather
//! than falling through to whatever sits behind the modal. Content-first
//! routing is unchanged — a focused field inside the panel sees every
//! keystroke, and only what it declines reaches the barrier, where Escape
//! dismisses and everything else is absorbed.
//!
//! # Entrance motion
//!
//! [`ModalEntrance::FadeZoom`] is the source's
//! `data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 duration-200`: the
//! panel is composited through a [`PaintScene::push_layer`] at the ramp's alpha
//! and drawn under a `0.95 → 1.0` scale about its own centre.
//! [`ModalEntrance::Slide`] is the sheet/drawer's `slide-in-from-<side>`, driven
//! in **layout** (the panel's origin moves) so hit-testing follows the panel
//! rather than lagging behind a paint-only transform — which is also why a
//! running slide asks for `request_layout`, not a bare `request_frame`. The
//! scrim fades with either.
//!
//! `Theme.motion.reduce_motion` collapses both to a jump: the ramp is stopped
//! and progress snaps to its rest value on the first paint, one frame after the
//! modal mounts (the pass that sets it is a paint, and the geometry it feeds is
//! a layout) — so a reduced-motion modal appears whole on its second frame
//! rather than animating on its first.
//!
//! # Exit motion, and why the close hook is state-free
//!
//! Every dismiss trigger — scrim tap, Escape, the close X, the handle, a
//! drag past its threshold — stages an **exit** instead of firing the app's
//! dismissal on the spot: the same `progress` ramp runs back down to `0.0`
//! (fade-zoom reverses its alpha and scale, a slide reverses its edge offset,
//! the scrim fades with either), the barrier keeps swallowing input the whole
//! way, a second trigger mid-exit is a no-op, and only when the ramp settles is
//! the dismissal fired — from `paint`, which is sound because
//! [`NavigatorController::pop`] merely *enqueues* an op applied on the next
//! rebuild (the `frust_material::sheet`/`frust_glyph::dialog` precedent; the
//! same paint asks for one more frame so that rebuild is guaranteed to come).
//!
//! Firing from paint is why the staged path's callback is
//! [`ModalView::on_close`] — a plain `Fn()` — and not the `Fn(&mut State)`
//! [`ModalView::on_dismiss`] takes: `PaintCtx` carries no app state.
//! [`show_modal`] wires `on_close` to `controller.pop()` for every component,
//! so the navigator path animates out with no app involvement, and app state
//! rides the navigator's own `on_result` (delivered with `&mut State` after the
//! pop) exactly as before. **A modal with no `on_close` wired** — a `Stack`
//! mount that only set `on_dismiss` — keeps the immediate, unstaged dismissal:
//! there is no state-bearing pass to defer into, so staging one would leave an
//! invisible barrier standing. `reduce_motion` collapses the exit the same way
//! it collapses the entrance: progress jumps to `0.0` on the next paint and the
//! close fires there, with no ramp.
//!
//! # Drag-to-close and snap points (the drawer's gesture)
//!
//! [`ModalConfig::drag`] turns an edge-pinned panel into vaul's draggable
//! drawer: a press on the handle — or anywhere on the panel the content did not
//! take — captures, each move scrubs `progress` along the panel's own axis so
//! the panel tracks the pointer (the scrim dims with it), and the release picks
//! the nearest resting point, with a fast flick ([`FLING_VELOCITY`]) nudging to
//! the next one in the direction of travel. Landing on `0.0` continues into the
//! exit ramp above rather than snapping shut. A press that never passes
//! [`TOUCH_SLOP`] is a *tap*: on the handle it dismisses, anywhere else it is
//! swallowed like any other barrier press.
//!
//! [`ModalView::snap_points`] is Base UI's contract: a value in `0..=1` is a
//! fraction of the panel's own extent, a value above `1` is logical px. The
//! drawer opens to the **first** point (fully open with none given), and the
//! resting set the release snaps to is those points plus `0.0` (closed) — so
//! with no snap points at all the two candidates are `0.0` and `1.0` and
//! "nearest" is exactly the conventional drag-past-the-midpoint commit.
//! The panel is laid out at its full extent and translated to expose the
//! active fraction (vaul's own transform model), so a partially-open drawer
//! carries a proportionally lighter scrim rather than a full-strength one.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CursorIcon, ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size,
    ThemeTextColor, View, Widget, any, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child, text::FontWeight, visit_children,
};
use frust::input::{TOUCH_SLOP, VelocityTracker};
use frust::{
    AnimationController, CrossAxisAlignment, Curve, EdgeInsets, FlexChild, FlexView, FrameTime,
    NavigatorController, Padding, PopResult, SizedBox, Theme, TransitionSpec, flexible, inflexible,
    text,
};
use kurbo::RoundedRectRadii;

use super::{OverlaySide, finite_or_zero};
use crate::hit::presses;
use crate::style::{self, PATH_TOLERANCE, ShadcnShadow};
use crate::tokens::ShadcnTokens;

/// `max-w-lg` — the dialog/command panel's cap, in logical px (Tailwind
/// `32rem`).
pub const MAX_WIDTH_LG: f64 = 512.0;
/// `max-w-xs` — the `sm` alert-dialog's cap, in logical px (Tailwind `20rem`).
pub const MAX_WIDTH_XS: f64 = 320.0;
/// `sm:max-w-sm` — a side sheet/drawer's cap, in logical px (Tailwind `24rem`).
pub const MAX_WIDTH_SM: f64 = 384.0;
/// `max-w-[calc(100%-2rem)]` — the viewport margin a centred panel keeps on
/// *each* side is half of this `2rem`.
const CENTERED_MARGIN: f64 = 16.0;
/// `w-3/4` — a side sheet/drawer's share of the viewport width.
pub const EDGE_FRACTION: f64 = 0.75;
/// `max-h-[80vh]` — a drawer's share of the viewport height.
pub const DRAWER_MAX_HEIGHT_FRACTION: f64 = 0.8;

/// `duration-200` on the dialog/alert-dialog/command content.
const FADE_ZOOM_MS: u64 = 200;
/// `data-[state=open]:duration-500` on the sheet content; the drawer's own
/// duration is vaul's rather than a class, and this port uses the same value.
const SLIDE_MS: u64 = 500;
/// `zoom-in-95`: the scale a fade-zoom entrance starts from.
const ZOOM_FROM: f64 = 0.95;
/// Progress difference below which a ramp counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

/// Release speed, in logical px/s along the drag axis, at or above which a
/// drawer drag commits in the direction it was flung rather than to whichever
/// resting point is nearest.
///
/// **Community-approximate**: the drag gesture is vaul's runtime behavior, not
/// a Tailwind class, so the vendored class list pins no value for it. 400 px/s
/// (0.4 px/ms) is a deliberate flick — an order of magnitude above
/// `frust::input::FLING_STOP`'s 30 px/s "this fling is over" floor, and well
/// under the speed of a full-screen swipe.
pub const FLING_VELOCITY: f64 = 400.0;

/// `top-4 right-4` — the close button's inset from the panel's top/right edges.
const CLOSE_INSET: f64 = 16.0;
/// The close button's **hit** box, in logical px.
///
/// The X itself is the source's `size-4` ([`style::ICON_SIZE`]); the hit box is
/// widened to `size-6` around it, centred, because a 16px target is unusable by
/// touch and the widening is invisible. Chrome-only — it changes no layout and
/// no painted pixel.
const CLOSE_HIT: f64 = 24.0;
/// `opacity-70` — the close X's rest opacity.
const CLOSE_OPACITY: f32 = 0.7;

/// The drag handle's `h-2`.
const HANDLE_HEIGHT: f64 = 8.0;
/// The drag handle's `w-[100px]`.
const HANDLE_WIDTH: f64 = 100.0;
/// The drag handle's `mt-4` from the panel's top edge.
const HANDLE_INSET: f64 = 16.0;
/// The handle's hit box height (`h-2` is 8px — too thin to press by touch; the
/// same chrome-only widening [`CLOSE_HIT`] documents).
const HANDLE_HIT_HEIGHT: f64 = 24.0;

/// Lucide's icon viewBox edge; its stroke width in the same units.
const LUCIDE_VIEWBOX: f64 = 24.0;
/// Lucide's nominal stroke width, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;
/// How far outside the panel its shadow may reach, in logical px — the bound the
/// fade layer is inflated by so `shadow-lg` is not clipped out of it
/// ([`ShadcnShadow`]'s `y_offset` plus a few standard deviations of blur).
const SHADOW_SPILL: f64 = 48.0;

/// How much of an axis a panel takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModalExtent {
    /// A fraction of the area's own extent on that axis (`w-3/4`).
    Fraction(f64),
    /// Whatever the content asks for (`h-auto`).
    Content,
}

/// An upper bound on a resolved [`ModalExtent`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModalLimit {
    /// No cap beyond the area itself.
    None,
    /// A fixed cap in logical px (`sm:max-w-sm`).
    Px(f64),
    /// A cap as a fraction of the area's extent (`max-h-[80vh]`).
    Fraction(f64),
}

impl ModalLimit {
    /// Apply this cap to `value`, given the area's extent on the same axis.
    fn apply(self, value: f64, area: f64) -> f64 {
        let cap = match self {
            ModalLimit::None => area,
            ModalLimit::Px(px) => px.min(area),
            ModalLimit::Fraction(f) => area * f,
        };
        value.min(cap)
    }
}

/// Where the panel sits inside the host area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModalGeometry {
    /// `top-1/2 left-1/2 -translate-1/2 w-full max-w-[calc(100%-2rem)]` with a
    /// per-component `sm:max-w-*` cap: centred both ways, content-height.
    Centered {
        /// The `sm:max-w-*` cap, in logical px.
        max_width: f64,
    },
    /// Pinned to one edge, full-bleed on the cross axis: the sheet's
    /// `inset-y-0 right-0 h-full w-3/4` and the drawer's
    /// `inset-x-0 bottom-0 h-auto max-h-[80vh]`.
    Edge {
        /// The edge the panel is pinned to (and slides in from).
        side: OverlaySide,
        /// The panel's extent *along* that edge's axis (its thickness).
        extent: ModalExtent,
        /// The cap on that extent.
        limit: ModalLimit,
    },
}

/// Which of the panel's corners are rounded (all at `rounded-lg`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalCorners {
    /// `rounded-lg` — the dialog/alert-dialog/command panel.
    #[default]
    All,
    /// Square — the sheet panel, which rounds nothing.
    None,
    /// `rounded-t-lg` — a bottom drawer.
    Top,
    /// `rounded-b-lg` — a top drawer.
    Bottom,
}

impl ModalCorners {
    /// The per-corner radii, clockwise from the top-left, for a `radius`-sized
    /// `rounded-lg`.
    fn radii(self, radius: f64) -> RoundedRectRadii {
        let (tl, tr, br, bl) = match self {
            ModalCorners::All => (radius, radius, radius, radius),
            ModalCorners::None => (0.0, 0.0, 0.0, 0.0),
            ModalCorners::Top => (radius, radius, 0.0, 0.0),
            ModalCorners::Bottom => (0.0, 0.0, radius, radius),
        };
        RoundedRectRadii::new(tl, tr, br, bl)
    }
}

/// Which of the panel's edges carry the 1px border.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalBorder {
    /// `border` — all four edges (the dialog family).
    #[default]
    All,
    /// One edge only: the sheet's `border-l`/`border-r`/`border-t`/`border-b`,
    /// always the panel's *inner* edge.
    Edge(OverlaySide),
    /// No border at all.
    None,
}

/// How the panel enters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalEntrance {
    /// `fade-in-0 zoom-in-95 duration-200` — paint-only.
    #[default]
    FadeZoom,
    /// `slide-in-from-<side>` — driven in layout, off the geometry's own edge.
    Slide,
    /// No entrance: the panel is at rest on its first frame.
    None,
}

impl ModalEntrance {
    /// This entrance's ramp duration.
    fn duration(self) -> Duration {
        match self {
            ModalEntrance::FadeZoom => Duration::from_millis(FADE_ZOOM_MS),
            ModalEntrance::Slide => Duration::from_millis(SLIDE_MS),
            ModalEntrance::None => Duration::ZERO,
        }
    }

    /// This entrance's easing: the source's `ease-out` for the zoom,
    /// `ease-in-out` for the slide.
    fn curve(self) -> Curve {
        match self {
            ModalEntrance::Slide => Curve::EaseInOut,
            _ => Curve::EaseOut,
        }
    }

    /// Whether the entrance moves the panel's *geometry* (and therefore needs a
    /// relayout per frame rather than a repaint).
    fn is_layout_affecting(self) -> bool {
        matches!(self, ModalEntrance::Slide)
    }
}

/// The accessibility role the modal reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalRole {
    /// A dialog box (`Role::Dialog`).
    #[default]
    Dialog,
    /// An alert dialog demanding a response (`Role::AlertDialog`).
    AlertDialog,
}

impl ModalRole {
    /// The accesskit role.
    fn role(self) -> Role {
        match self {
            ModalRole::Dialog => Role::Dialog,
            ModalRole::AlertDialog => Role::AlertDialog,
        }
    }
}

/// The chrome differences between the five modal components — see the [module
/// docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModalConfig {
    /// Where the panel sits.
    pub geometry: ModalGeometry,
    /// Which corners round.
    pub corners: ModalCorners,
    /// Which edges carry the border.
    pub border: ModalBorder,
    /// The panel's shadow rung, if any (`shadow-lg` on the dialog family and the
    /// sheet; the drawer's own class list carries none).
    pub shadow: Option<ShadcnShadow>,
    /// How the panel enters.
    pub entrance: ModalEntrance,
    /// Whether a press+release outside the panel dismisses.
    pub scrim_dismiss: bool,
    /// Whether the `top-4 right-4` close X is shown.
    pub close_button: bool,
    /// Whether the drag-handle bar is shown (and dismisses on release).
    pub handle: bool,
    /// Whether the panel can be dragged along its own edge axis to close — the
    /// drawer's gesture (see the [module docs](self)). Meaningful only for a
    /// [`ModalGeometry::Edge`] panel; the dialog family and the sheet leave it
    /// off, matching their Radix-dialog sources.
    pub drag: bool,
    /// The accessibility role.
    pub role: ModalRole,
}

impl ModalConfig {
    /// The dialog family's chrome: a centred `rounded-lg border shadow-lg`
    /// panel capped at `max_width`, fading and zooming in.
    pub fn centered(max_width: f64) -> Self {
        ModalConfig {
            geometry: ModalGeometry::Centered { max_width },
            corners: ModalCorners::All,
            border: ModalBorder::All,
            shadow: Some(style::SHADOW_LG),
            entrance: ModalEntrance::FadeZoom,
            scrim_dismiss: true,
            close_button: false,
            handle: false,
            drag: false,
            role: ModalRole::Dialog,
        }
    }

    /// The sheet's chrome on `side`: a square-cornered `shadow-lg` panel pinned
    /// to that edge, bordered on its inner edge, sliding in.
    ///
    /// A left/right sheet is `w-3/4 sm:max-w-sm` and full height; a top/bottom
    /// one is `h-auto` and full width — the two shapes `sheet.tsx`'s own side
    /// branches spell out.
    pub fn edge(side: OverlaySide) -> Self {
        let (extent, limit) = if side.is_vertical() {
            (ModalExtent::Content, ModalLimit::None)
        } else {
            (
                ModalExtent::Fraction(EDGE_FRACTION),
                ModalLimit::Px(MAX_WIDTH_SM),
            )
        };
        ModalConfig {
            geometry: ModalGeometry::Edge {
                side,
                extent,
                limit,
            },
            corners: ModalCorners::None,
            border: ModalBorder::Edge(side.opposite()),
            shadow: Some(style::SHADOW_LG),
            entrance: ModalEntrance::Slide,
            scrim_dismiss: true,
            close_button: false,
            handle: false,
            drag: false,
            role: ModalRole::Dialog,
        }
    }

    /// Replace the panel's extent along its own axis (an [`ModalGeometry::Edge`]
    /// config only; a no-op on a centred one).
    pub fn extent(mut self, extent: ModalExtent, limit: ModalLimit) -> Self {
        if let ModalGeometry::Edge {
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
    pub fn corners(mut self, corners: ModalCorners) -> Self {
        self.corners = corners;
        self
    }

    /// Set which edges carry the border.
    pub fn border(mut self, border: ModalBorder) -> Self {
        self.border = border;
        self
    }

    /// Set (or clear) the panel's shadow rung.
    pub fn shadow(mut self, shadow: Option<ShadcnShadow>) -> Self {
        self.shadow = shadow;
        self
    }

    /// Set the entrance.
    pub fn entrance(mut self, entrance: ModalEntrance) -> Self {
        self.entrance = entrance;
        self
    }

    /// Enable or disable scrim-tap dismissal.
    pub fn scrim_dismiss(mut self, scrim_dismiss: bool) -> Self {
        self.scrim_dismiss = scrim_dismiss;
        self
    }

    /// Show or hide the close X.
    pub fn close_button(mut self, close_button: bool) -> Self {
        self.close_button = close_button;
        self
    }

    /// Show or hide the drag handle.
    pub fn handle(mut self, handle: bool) -> Self {
        self.handle = handle;
        self
    }

    /// Enable or disable drag-to-close (see [`ModalConfig::drag`]).
    pub fn drag(mut self, drag: bool) -> Self {
        self.drag = drag;
        self
    }

    /// Set the accessibility role.
    pub fn role(mut self, role: ModalRole) -> Self {
        self.role = role;
        self
    }
}

/// A view-held, typed dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A state-free close hook, fired from `paint` when an exit ramp settles (see
/// the [module docs](self)).
pub type OnClose = Rc<dyn Fn()>;

/// A modal component that [`show_modal`] can wire a navigator pop into.
///
/// Every modal builder in the catalog implements it by storing the callback in
/// its own `on_dismiss` slot; the trait exists so one push helper serves all
/// five rather than each component re-deriving the same
/// `push_transparent_for_result` call.
///
/// Its element is the shared [`ModalWidget`] by definition — a modal component
/// *is* a pre-configured modal host — which is what lets [`show_modal`] install
/// the staged-exit close hook on whatever component it was handed without a
/// per-component setter.
pub trait ModalContent<State: 'static>:
    View<State, Element = ModalWidget> + Sized + 'static
{
    /// Install the dismiss callback, replacing any the builder already set.
    fn on_modal_dismiss(self, on_dismiss: OnDismiss<State>) -> Self;
}

/// Build a modal host: `content` inside a panel shaped by `config`, over
/// shadcn's `bg-black/50` scrim.
///
/// The content view owns all of the panel's padding and layout — the panel
/// itself contributes only chrome (fill, border, corners, shadow) plus the
/// optional close button and drag handle, which sit *over* the content the way
/// their `absolute` upstream counterparts do.
pub fn modal<State: 'static, V: View<State>>(content: V, config: ModalConfig) -> ModalView<State> {
    ModalView {
        content: any(content),
        config,
        label: None,
        on_dismiss: None,
        on_close: None,
        snap_points: Vec::new(),
    }
}

/// A declarative modal host. See [`modal`].
///
/// The fields are crate-visible so the catalog's five modal components can wrap
/// one and adjust its config in their own builders (a `DialogView` *is* a
/// pre-configured `ModalView`); an app outside the crate configures it through
/// [`modal`]'s `config` argument and the builders below.
pub struct ModalView<State: 'static> {
    pub(crate) content: AnyView<State>,
    pub(crate) config: ModalConfig,
    pub(crate) label: Option<String>,
    pub(crate) on_dismiss: Option<OnDismiss<State>>,
    pub(crate) on_close: Option<OnClose>,
    pub(crate) snap_points: Vec<f64>,
}

impl<State: 'static> ModalView<State> {
    /// Label the modal's accessibility node (its title text).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the **unstaged** dismiss callback — a scrim tap, the close X, the
    /// handle, or Escape, delivered with `&mut State` during the event pass.
    ///
    /// Used only when no [`on_close`](Self::on_close) hook is wired: with one,
    /// the dismissal is staged behind the exit ramp and fired from `paint`,
    /// where no app state exists (see the [module docs](self)).
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Set the state-free close hook fired when the exit ramp settles — the
    /// staged-dismissal path. [`show_modal`] wires this to `controller.pop()`;
    /// a `Stack`-mounted modal wires its own (a signal write, a navigator pop).
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// Set the drawer's snap points: `0..=1` a fraction of the panel's own
    /// extent, above `1` logical px (Base UI's contract — see the
    /// [module docs](self)). The panel opens to the first point.
    pub fn snap_points(mut self, points: &[f64]) -> Self {
        self.snap_points = points.to_vec();
        self
    }
}

impl<State: 'static> ModalContent<State> for ModalView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: OnDismiss<State>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }
}

/// Push `build`'s modal as a transparent navigator page and register
/// `on_result` for the value it pops with — the shared push behind every
/// component's `show_*` wrapper.
///
/// The modal's dismissal is wired to `controller.pop()` (an *empty*
/// [`PopResult`], overriding any `on_dismiss` the builder set); an action inside
/// the content pops with a value via `controller.pop_with_result(..)`. The
/// navigator transition is [`TransitionSpec::NONE`] on purpose: the modal stages
/// its own entrance *and exit* ([`ModalEntrance`]), so a page transition on top
/// of it would animate the same thing twice.
///
/// The pop is wired **twice, by design**: as the staged
/// [`ModalView::on_close`] hook the exit ramp fires on settle (the path every
/// dismissal actually takes here), and as the unstaged
/// [`ModalContent::on_modal_dismiss`] callback, which only runs for a config
/// with no entrance to reverse. Exactly one of the two fires per dismissal.
pub fn show_modal<State, V, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    V: ModalContent<State>,
    B: Fn() -> V + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss_ctrl = controller.clone();
    let close_ctrl = controller.clone();
    controller.push_transparent_for_result(
        move || {
            let ctrl = dismiss_ctrl.clone();
            let close = close_ctrl.clone();
            any::<State, _>(StagedExit {
                inner: build().on_modal_dismiss(Rc::new(move |_state: &mut State| ctrl.pop())),
                on_close: Rc::new(move || close.pop()) as OnClose,
            })
        },
        TransitionSpec::NONE,
        on_result,
    );
}

/// A modal component with the staged-exit close hook installed on the
/// [`ModalWidget`] it builds.
///
/// [`show_modal`] is generic over all five modal components, so it cannot reach
/// the [`ModalView`] each of them wraps privately; every one of them *does*
/// build the shared [`ModalWidget`] ([`ModalContent`]'s element bound), so this
/// thin pass-through installs the hook on the built widget instead — the same
/// place [`ModalView`]'s own `on_close` lands, and last writer wins.
struct StagedExit<V> {
    inner: V,
    on_close: OnClose,
}

impl<State: 'static, V: View<State, Element = ModalWidget>> View<State> for StagedExit<V> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        let mut widget = self.inner.build(ctx);
        widget.on_close = Some(self.on_close.clone());
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = self.inner.rebuild(&prev.inner, element, ctx);
        element.on_close = Some(self.on_close.clone());
        flags
    }

    fn teardown(&self, element: &mut ModalWidget, ctx: &mut BuildCtx<'_>) {
        self.inner.teardown(element, ctx);
    }
}

/// Interleave `children` with `gap`-sized spacers, as inflexible flex children.
///
/// `FlexView` v1 has no `gap` property (only `MainAxisAlignment::Start`), so
/// every gap in the catalog is an explicit [`SizedBox`] spacer — the same
/// stand-in `crate::card` documents.
fn with_gaps<State: 'static>(
    children: Vec<AnyView<State>>,
    gap: f64,
    vertical: bool,
) -> Vec<FlexChild<State>> {
    let mut out: Vec<FlexChild<State>> = Vec::with_capacity(children.len() * 2);
    for (i, child) in children.into_iter().enumerate() {
        if i > 0 {
            out.push(inflexible(if vertical {
                SizedBox(None, Some(gap))
            } else {
                SizedBox(Some(gap), None)
            }));
        }
        out.push(inflexible(child));
    }
    out
}

/// A `flex flex-col gap-<gap> p-<pad>` stack, stretched to the panel's width —
/// the shape of every modal panel's own content column *and* of each of its
/// header slots.
pub fn stack_slots<State: 'static>(
    children: Vec<AnyView<State>>,
    gap: f64,
    pad: EdgeInsets,
) -> AnyView<State> {
    any(Padding(
        pad,
        FlexView::new(frust::Axis::Vertical, with_gaps(children, gap, true))
            .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

/// A trailing-aligned row, `gap` apart — the dialog/alert-dialog footer's
/// `sm:flex-row sm:justify-end`.
///
/// `justify-end` is a flexible *leading* spacer here: `MainAxisAlignment` v1
/// packs at the start only, so the free space is given to a zero-size first
/// child instead.
///
/// # Breakpoint arm
///
/// shadcn authors these footers mobile-first (`flex-col-reverse`) with a `sm:`
/// desktop arm. This port has no breakpoint mechanism and renders the **desktop
/// arm** at every width, matching the catalog's desktop-first charter — the
/// same reason its headers stay start-aligned where upstream centres them below
/// `sm`.
pub fn trailing_row<State: 'static>(
    children: Vec<AnyView<State>>,
    gap: f64,
    pad: EdgeInsets,
) -> AnyView<State> {
    let mut inner: Vec<FlexChild<State>> = vec![flexible(1, SizedBox(None, None))];
    inner.extend(with_gaps(children, gap, false));
    any(Padding(
        pad,
        FlexView::new(frust::Axis::Horizontal, inner).cross_axis(CrossAxisAlignment::Center),
    ))
}

/// A panel title: `font-semibold` at `size`, in the panel's own ink
/// (`text-foreground`).
pub fn panel_title<State: 'static>(title: impl Into<String>, size: f64) -> AnyView<State> {
    any(text(title)
        .size(size as f32)
        .weight(FontWeight::SEMI_BOLD)
        .themed_role(ThemeTextColor::OnSurface))
}

/// A panel description: `text-sm text-muted-foreground`.
pub fn panel_description<State: 'static>(description: impl Into<String>) -> AnyView<State> {
    any(text(description)
        .size(style::TEXT_SM as f32)
        .themed_role(ThemeTextColor::OnSurfaceVariant))
}

/// The vertical space a drag handle occupies at the top of a drawer panel
/// (`mt-4 h-2`) — the inset a drawer's own content reserves for it, since the
/// handle is painted as panel chrome rather than laid out as a child.
pub const HANDLE_RESERVE: f64 = HANDLE_INSET + HANDLE_HEIGHT;

/// The retained widget for a [`ModalView`].
pub struct ModalWidget {
    content: ChildPod,
    config: ModalConfig,
    label: Option<String>,
    on_dismiss: Option<ErasedCallback>,
    /// The state-free close hook the exit ramp fires on settle (see the module
    /// docs); `None` leaves dismissal unstaged.
    on_close: Option<OnClose>,
    /// The drawer's resting points, as authored (see [`ModalView::snap_points`]).
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
    /// A scrim/panel-background press is in flight (the modal barrier).
    scrim_captured: bool,
    /// Whether that press started outside the panel — only an outside press
    /// released outside dismisses.
    scrim_down_outside: bool,
    /// The close button's latched hover/press state.
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

impl ModalWidget {
    /// The panel rect in the widget's own coordinate space.
    pub fn panel_rect(&self) -> Rect {
        self.panel
    }

    /// The close button's hit box, when the config shows one.
    pub fn close_rect(&self) -> Option<Rect> {
        self.config.close_button.then(|| {
            let center = Point::new(
                self.panel.x1 - CLOSE_INSET - style::ICON_SIZE / 2.0,
                self.panel.y0 + CLOSE_INSET + style::ICON_SIZE / 2.0,
            );
            Rect::from_center_size(center, Size::new(CLOSE_HIT, CLOSE_HIT))
        })
    }

    /// The drag handle's hit box, when the config shows one.
    pub fn handle_rect(&self) -> Option<Rect> {
        self.config.handle.then(|| {
            let center = Point::new(
                self.panel.center().x,
                self.panel.y0 + HANDLE_INSET + HANDLE_HEIGHT / 2.0,
            );
            Rect::from_center_size(center, Size::new(HANDLE_WIDTH, HANDLE_HIT_HEIGHT))
        })
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

    /// The progress an open panel rests at: the first snap point, or fully
    /// open with none authored.
    pub fn open_progress(&self) -> f64 {
        self.snap_points
            .first()
            .map_or(1.0, |p| self.normalize_snap(*p))
    }

    /// A snap point in progress space: `0..=1` is already a fraction of the
    /// panel's extent, anything larger is logical px against that extent
    /// (Base UI's contract). Clamped into `0..=1`, and treated as fully open
    /// while no extent has been laid out yet.
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
            ModalGeometry::Edge { side, .. } if side.is_vertical() => self.panel.height(),
            ModalGeometry::Edge { .. } => self.panel.width(),
            ModalGeometry::Centered { .. } => 0.0,
        }
    }

    /// The panel's slide axis, and which way along it closes: `+1.0` when the
    /// panel leaves toward growing coordinates (bottom/right), `-1.0` otherwise.
    fn close_sign(&self) -> f64 {
        match self.config.geometry {
            ModalGeometry::Edge {
                side: OverlaySide::Bottom | OverlaySide::Right,
                ..
            } => 1.0,
            _ => -1.0,
        }
    }

    /// `position` projected onto the panel's slide axis.
    fn axis_pos(&self, position: Point) -> f64 {
        match self.config.geometry {
            ModalGeometry::Edge { side, .. } if side.is_vertical() => position.y,
            _ => position.x,
        }
    }

    /// The last painted frame time in milliseconds — the event pass's clock,
    /// since a pointer event carries none.
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Whether this modal can be dismissed at all (either channel wired).
    fn dismissable(&self) -> bool {
        self.on_dismiss.is_some() || self.on_close.is_some()
    }

    /// Start a ramp from the current progress to `to`, over the entrance's own
    /// duration scaled by how much of the travel is left (a half-open panel
    /// closes in half the time) and eased by its own curve.
    fn begin_ramp(&mut self, to: f64, exiting: bool) {
        let from = self.progress;
        self.ramp = (from, to);
        self.exiting = exiting;
        let fraction = (to - from).abs().clamp(0.0, 1.0);
        let duration = self.config.entrance.duration().mul_f64(fraction);
        self.anim = AnimationController::new(duration).with_curve(self.config.entrance.curve());
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

    /// Stage the dismissal: run the entrance ramp backwards and fire the close
    /// hook when it settles.
    ///
    /// Falls back to firing [`ModalView::on_dismiss`] on the spot when there is
    /// nothing to stage — no close hook to fire from `paint`, or no entrance to
    /// reverse. A trigger arriving while an exit is already running is a no-op.
    fn request_dismiss(&mut self, ctx: &mut EventCtx) {
        if self.exiting {
            return;
        }
        let stageable = self.on_close.is_some() && self.config.entrance != ModalEntrance::None;
        if !stageable {
            if let Some(on_dismiss) = self.on_dismiss.as_mut() {
                on_dismiss(ctx);
            } else if let Some(on_close) = &self.on_close {
                on_close();
            }
            return;
        }
        self.begin_ramp(0.0, true);
        ctx.request_redraw();
    }

    /// Move the panel to `progress` without animating — the drag scrub. The
    /// panel keeps the extent layout gave it and only its origin moves, so no
    /// relayout is owed.
    ///
    /// A live drag takes the panel over from whatever ramp was running,
    /// including an exit: catching a closing drawer cancels its dismissal, and
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

    /// Re-place an edge-pinned panel (and its content) for the current
    /// progress, inside an area of `area`.
    fn reposition(&mut self, area: Size) {
        let ModalGeometry::Edge { side, .. } = self.config.geometry else {
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
    /// re-baselining would leave the panel trailing the finger by 18px for the
    /// rest of the gesture.
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
        self.scrub(ctx, target.clamp(0.0, open));
    }

    /// The resting point a release settles to: the nearest one, or — past
    /// [`FLING_VELOCITY`] — the next one along the direction of travel.
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
        if velocity.abs() >= FLING_VELOCITY {
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
    fn settle_drag(&mut self, ctx: &mut EventCtx, target: f64) {
        if target <= PROGRESS_EPSILON {
            self.request_dismiss(ctx);
        } else {
            self.begin_ramp(target, false);
            ctx.request_redraw();
        }
    }

    /// The handle's painted bar (a subset of its hit box — see [`CLOSE_HIT`]).
    fn handle_bar(&self) -> Rect {
        Rect::from_center_size(
            Point::new(
                self.panel.center().x,
                self.panel.y0 + HANDLE_INSET + HANDLE_HEIGHT / 2.0,
            ),
            Size::new(HANDLE_WIDTH, HANDLE_HEIGHT),
        )
    }
}

impl<State: 'static> View<State> for ModalView<State> {
    type Element = ModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ModalWidget {
        let settled = self.config.entrance == ModalEntrance::None;
        ModalWidget {
            content: build_child(&self.content, ctx),
            config: self.config,
            label: self.label.clone(),
            on_dismiss: self.on_dismiss.as_ref().map(erase_callback),
            on_close: self.on_close.clone(),
            snap_points: self.snap_points.clone(),
            panel: Rect::ZERO,
            anim: AnimationController::new(self.config.entrance.duration())
                .with_curve(self.config.entrance.curve()),
            // The entrance's target resolves on the first paint, not here: a
            // snap point in logical px needs the extent layout has yet to
            // measure.
            ramp: (0.0, 1.0),
            progress: if settled { 1.0 } else { 0.0 },
            started: settled,
            exiting: false,
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
        element: &mut ModalWidget,
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
        if element.snap_points != self.snap_points {
            element.snap_points = self.snap_points.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures aren't comparable, so both dismiss adapters are reinstalled
        // unconditionally (cheap — what every interactive widget does). A
        // `None` here never clears a hook `show_modal`'s own pass-through
        // installed: that wrapper reinstalls after this rebuild returns.
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

/// Lay the panel out and place the content inside it.
///
/// Split out of [`Widget::layout`] so the geometry is one readable expression
/// per [`ModalGeometry`] arm; returns the panel rect (already offset by the
/// slide progress, if any).
fn layout_panel(
    content: &mut ChildPod,
    ctx: &mut LayoutCtx,
    config: ModalConfig,
    area: Size,
    progress: f64,
) -> Rect {
    match config.geometry {
        ModalGeometry::Centered { max_width } => {
            let width = max_width.min((area.width - 2.0 * CENTERED_MARGIN).max(0.0));
            let max_height = (area.height - 2.0 * CENTERED_MARGIN).max(0.0);
            // `w-full` inside the cap: a tight width, a loose height, exactly
            // what the source's `grid` content resolves to.
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
        ModalGeometry::Edge {
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
                ModalExtent::Fraction(f) => limit.apply(axis_area * f, axis_area),
                ModalExtent::Content => {
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
            if extent != ModalExtent::Content {
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

/// Stroke the panel's border per [`ModalBorder`].
fn paint_border(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radii: RoundedRectRadii,
    border: ModalBorder,
    color: Color,
) {
    match border {
        ModalBorder::None => {}
        ModalBorder::All => {
            let half = style::BORDER_WIDTH / 2.0;
            let rr = RoundedRect::from_rect(
                Rect::new(half, half, size.width - half, size.height - half),
                radii,
            );
            scene.stroke_path(
                origin,
                &rr.to_path(PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(color),
            );
        }
        ModalBorder::Edge(edge) => {
            let half = style::BORDER_WIDTH / 2.0;
            let (from, to) = match edge {
                OverlaySide::Top => (Point::new(0.0, half), Point::new(size.width, half)),
                OverlaySide::Bottom => (
                    Point::new(0.0, size.height - half),
                    Point::new(size.width, size.height - half),
                ),
                OverlaySide::Left => (Point::new(half, 0.0), Point::new(half, size.height)),
                OverlaySide::Right => (
                    Point::new(size.width - half, 0.0),
                    Point::new(size.width - half, size.height),
                ),
            };
            let mut path = BezPath::new();
            path.move_to(from);
            path.line_to(to);
            scene.stroke_path(origin, &path, style::BORDER_WIDTH, &Brush::Solid(color));
        }
    }
}

/// Paint lucide's `x` (`M18 6 6 18` + `m6 6 12 12`) centred on `center`,
/// `extent` px on a side.
fn draw_x(scene: &mut dyn PaintScene, center: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    // The two strokes, viewBox-relative to its centre (12, 12).
    let arms = [((-6.0, -6.0), (6.0, 6.0)), ((-6.0, 6.0), (6.0, -6.0))];
    let mut path = BezPath::new();
    for ((x0, y0), (x1, y1)) in arms {
        path.move_to(Point::new(x0 * scale, y0 * scale));
        path.line_to(Point::new(x1 * scale, y1 * scale));
    }
    scene.stroke_path(center, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Whether `pos` falls inside an optional hit box (`false` when the box is not
/// shown at all).
fn hit(rect: Option<Rect>, pos: Point) -> bool {
    rect.is_some_and(|r| r.contains(pos))
}

impl Widget for ModalWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        self.panel = layout_panel(&mut self.content, ctx, self.config, area, self.progress);
        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
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
        let (reduce_motion, radius, scrim, fill, border, ink, muted, shadow_color) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ShadcnTokens::resolve_radius(None, theme).lg,
                super::scrim(theme),
                super::background(theme),
                super::border(theme),
                super::foreground(theme),
                super::muted(theme),
                self.config.shadow.map(|s| s.color(theme)),
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
            if !reduce_motion {
                self.anim.forward();
            }
        }
        let next = if reduce_motion {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.ramp.1
        } else if self.anim.is_animating() {
            if self.anim.advance(now) {
                // `advance`'s first call after `forward()` only seeds the
                // clock (zero delta, but still truthy) — the continuation has
                // to be requested on every truthy advance, not just the ones
                // that moved `progress`, or a slide's seeding paint never
                // schedules the frame that would carry it off zero.
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
        // hook only *enqueues* a navigator pop (it writes no tracked signal),
        // so this paint asks for the frame whose rebuild drains it — otherwise
        // a `ControlFlow::Wait` desktop shell idles and the modal never leaves.
        if self.exiting && !self.anim.is_animating() && self.progress <= PROGRESS_EPSILON {
            self.exiting = false;
            self.progress = 0.0;
            if let Some(on_close) = &self.on_close {
                on_close();
                ctx.request_frame();
            }
        }
        let progress = self.progress;

        // The scrim fades with the panel (`data-[state=open]:fade-in-0` on the
        // overlay), and covers the whole area.
        scene.fill_rect(origin, size, style::scale_alpha(scrim, progress as f32));

        let panel_origin = Point::new(origin.x + self.panel.x0, origin.y + self.panel.y0);
        let panel_size = self.panel.size();
        let radii = self.config.corners.radii(radius);
        let fade_zoom = self.config.entrance == ModalEntrance::FadeZoom && progress < 1.0;
        if fade_zoom {
            // `fade-in-0`: composite the panel at the ramp's alpha. The layer
            // rect is inflated so the shadow, which reaches outside the panel,
            // is not clipped out of it.
            let layer = Rect::from_origin_size(panel_origin, panel_size)
                .inflate(SHADOW_SPILL, SHADOW_SPILL);
            scene.push_layer(layer.origin(), layer.size(), progress as f32);
            // …and `zoom-in-95`: scale about the panel's own centre.
            let c = Rect::from_origin_size(panel_origin, panel_size).center();
            let scale = ZOOM_FROM + (1.0 - ZOOM_FROM) * progress;
            scene.push_transform(
                Affine::translate(c.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-c.to_vec2()),
            );
        }

        if let (Some(shadow), Some(color)) = (self.config.shadow, shadow_color) {
            // The same offset rule `style::draw_shadow` applies, spelled out
            // here because that helper wants a live `&Theme` borrow this pass
            // has already released.
            scene.draw_shadow(
                Point::new(panel_origin.x, panel_origin.y + shadow.y_offset),
                panel_size,
                radius,
                shadow.std_dev,
                color,
            );
        }
        let panel_path =
            RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, panel_size), radii)
                .to_path(PATH_TOLERANCE);
        scene.fill_path(panel_origin, &panel_path, &Brush::Solid(fill));

        // The content is clipped to the panel: an `overflow-hidden` panel (the
        // command dialog) needs it, and it is what keeps a too-tall panel's
        // content inside the window.
        if self.config.corners == ModalCorners::All {
            scene.push_clip_rounded(panel_origin, panel_size, radius);
        } else {
            scene.push_clip(panel_origin, panel_size);
        }
        self.content.paint_child(ctx, scene);
        scene.pop_clip();

        paint_border(
            scene,
            panel_origin,
            panel_size,
            radii,
            self.config.border,
            border,
        );

        if self.config.close_button {
            // `opacity-70 hover:opacity-100`, over the panel's own ink.
            let alpha = if self.close_hovered {
                1.0
            } else {
                CLOSE_OPACITY
            };
            let icon_center = Point::new(
                origin.x + self.panel.x1 - CLOSE_INSET - style::ICON_SIZE / 2.0,
                origin.y + self.panel.y0 + CLOSE_INSET + style::ICON_SIZE / 2.0,
            );
            draw_x(
                scene,
                icon_center,
                style::ICON_SIZE,
                style::with_alpha(ink, alpha),
            );
        }
        if self.config.handle {
            let bar = self.handle_bar();
            scene.fill_rounded_rect(
                Point::new(origin.x + bar.x0, origin.y + bar.y0),
                bar.size(),
                bar.height() / 2.0,
                muted,
            );
        }

        if fade_zoom {
            scene.pop_transform();
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Claim focus on every `Down` — the `frust_material::dialog` opt-in that
        // makes Escape reachable, and what keeps the root's focus session alive
        // while the modal is up (re-claiming while focused is a no-op; a
        // claim-once guard would kill the session on the second tap).
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
            if key.key == Key::Named(NamedKey::Escape) && self.dismissable() {
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
        // no close-button press, no drag, no scrim-dismiss arming. Only the
        // primary button does any of that (the catalog's press rule), and the
        // panel's own content, routed above, has already had its chance at the
        // event (a context menu inside the panel still opens).
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
                        ctx.set_cursor(style::ACTIVE_CURSOR);
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
                        ctx.set_cursor(style::ACTIVE_CURSOR);
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
                        // A tap, not a drag: on the handle it dismisses (vaul's
                        // own click-to-close), on the panel it is an ordinary
                        // swallowed barrier press.
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
pub(crate) mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{KeyEvent, Modifiers, PointerButton, PointerEvent};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;
    use std::cell::Cell;

    /// The window every modal test lays out in.
    pub(crate) const WINDOW: Size = Size::new(400.0, 600.0);

    /// A recording `PaintScene` shared by the modal component tests: the fills,
    /// paths, strokes, clips, shadows and fade layers a modal emits.
    #[derive(Default)]
    pub(crate) struct Recorder {
        /// `fill_rect` calls — the scrim is the first of them.
        pub rects: Vec<(Point, Size, Color)>,
        /// `fill_path` calls, as `(origin, bounding box, color)` — the panel.
        pub paths: Vec<(Point, Rect, Color)>,
        /// `fill_rounded_rect` calls — the drag handle.
        pub rrects: Vec<(Point, Size, f64, Color)>,
        /// `stroke_path` calls, as `(bounding box, width, color)`.
        pub strokes: Vec<(Rect, f64, Color)>,
        /// `draw_shadow` calls.
        pub shadows: Vec<(Point, Size, f64, f64, Color)>,
        /// `push_clip`/`push_clip_rounded` calls.
        pub clips: Vec<(Point, Size)>,
        /// `push_layer` alphas — the fade.
        pub layers: Vec<f32>,
        /// `push_transform` affines — the zoom.
        pub transforms: Vec<Affine>,
        /// Shaped glyph runs, proving text children painted.
        pub glyphs: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
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
        fn pop_clip(&mut self) {}
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {}
        fn draw_glyph_run(&mut self, _run: frust::authoring::scene::GlyphRun) {
            self.glyphs += 1;
        }
    }

    /// A fixed-size leaf generic over the app state, for a modal's content.
    pub(crate) struct Block(pub Size);

    /// The retained half of [`Block`].
    pub(crate) struct BlockWidget(Size);

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
    pub(crate) fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A pointer event at `(x, y)`.
    pub(crate) fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// An Escape key event.
    pub(crate) fn escape() -> InputEvent {
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

    fn build(view: &ModalView<Flags>) -> ModalWidget {
        let mut counter = 0u64;
        View::<Flags>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ModalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    fn dispatch(w: &mut ModalWidget, state: &mut Flags, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    fn view(config: ModalConfig) -> ModalView<Flags> {
        modal(Block(Size::new(200.0, 120.0)), config).on_dismiss(|s: &mut Flags| s.dismissed += 1)
    }

    /// A modal wired the way [`show_modal`] wires one: a state-free close hook
    /// (here a counter) alongside the unstaged `on_dismiss`, so the staged exit
    /// path is the one taken.
    fn staged(config: ModalConfig) -> (ModalView<Flags>, Rc<Cell<u32>>) {
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let view = view(config).on_close(move || hook.set(hook.get() + 1));
        (view, closed)
    }

    /// Paint one frame at `ms` and return what it drew.
    fn frame(w: &mut ModalWidget, ms: f64) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        rec
    }

    /// Paint (and relay out) until the running ramp is over.
    fn run_ramp(w: &mut ModalWidget, from_ms: f64) {
        frame(w, from_ms);
        layout(w);
        frame(w, from_ms + 2000.0);
        layout(w);
    }

    fn settled(config: ModalConfig) -> ModalConfig {
        config.entrance(ModalEntrance::None)
    }

    #[test]
    fn a_centered_panel_is_capped_centred_and_content_tall() {
        let mut w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
        let size = layout(&mut w);
        assert_eq!(size, WINDOW, "the host fills its area (the scrim)");
        let panel = w.panel_rect();
        // `max-w-[calc(100%-2rem)]` bites before `max-w-lg` in a 400px window.
        assert_eq!(panel.width(), WINDOW.width - 2.0 * CENTERED_MARGIN);
        assert_eq!(panel.height(), 120.0, "content-tall");
        assert!((panel.center().x - WINDOW.width / 2.0).abs() < 1e-9);
        assert!((panel.center().y - WINDOW.height / 2.0).abs() < 1e-9);
        assert_eq!(w.content.origin(), panel.origin());
    }

    #[test]
    fn an_edge_panel_pins_to_its_side_at_three_quarters_capped_at_max_w_sm() {
        for side in [OverlaySide::Left, OverlaySide::Right] {
            let mut w = build(&view(settled(ModalConfig::edge(side))));
            layout(&mut w);
            let panel = w.panel_rect();
            assert_eq!(
                panel.width(),
                (WINDOW.width * EDGE_FRACTION).min(MAX_WIDTH_SM)
            );
            assert_eq!(panel.height(), WINDOW.height, "full height");
            match side {
                OverlaySide::Left => assert_eq!(panel.x0, 0.0),
                _ => assert_eq!(panel.x1, WINDOW.width),
            }
        }
        for side in [OverlaySide::Top, OverlaySide::Bottom] {
            let mut w = build(&view(settled(ModalConfig::edge(side))));
            layout(&mut w);
            let panel = w.panel_rect();
            assert_eq!(panel.width(), WINDOW.width, "full width");
            assert_eq!(panel.height(), 120.0, "`h-auto`");
            match side {
                OverlaySide::Top => assert_eq!(panel.y0, 0.0),
                _ => assert_eq!(panel.y1, WINDOW.height),
            }
        }
    }

    #[test]
    fn a_limit_caps_an_edge_panel() {
        let config = settled(ModalConfig::edge(OverlaySide::Bottom).extent(
            ModalExtent::Content,
            ModalLimit::Fraction(DRAWER_MAX_HEIGHT_FRACTION),
        ));
        let mut w = build(&modal(Block(Size::new(200.0, 5000.0)), config));
        layout(&mut w);
        assert_eq!(
            w.panel_rect().height(),
            WINDOW.height * DRAWER_MAX_HEIGHT_FRACTION
        );
    }

    /// The constraints `frust::scroll_view` hands its child: the viewport
    /// width, an infinite max height.
    fn scrolled_bc() -> BoxConstraints {
        BoxConstraints::new(
            Size::new(WINDOW.width, 0.0),
            Size::new(WINDOW.width, f64::INFINITY),
        )
    }

    #[test]
    fn an_unbounded_height_collapses_the_host_and_its_panel_to_nothing() {
        // The documented mounts (a navigator page, a full-area `Stack`) are
        // bounded; a host put inside a scroll view is not, and this is what
        // that costs — pinned, not fixed here: the coercion cannot invent an
        // extent nobody offered, so the fix belongs at the mount site.
        let mut w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &scrolled_bc());
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

    #[test]
    fn the_scrim_is_black_at_fifty_percent_and_the_panel_is_background() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let mut w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.rects[0].1, size, "the scrim covers the area");
        assert_eq!(rec.rects[0].2, theme.scheme().scrim);
        assert_eq!(rec.paths[0].2, theme.scheme().surface, "`bg-background`");
        assert_eq!(rec.strokes[0].2, theme.scheme().outline, "`border`");
        assert_eq!(rec.shadows[0].3, style::SHADOW_LG.std_dev, "`shadow-lg`");
        assert_eq!(rec.clips.len(), 1, "the content is clipped to the panel");
    }

    #[test]
    fn a_scrim_press_and_release_outside_the_panel_dismisses() {
        let mut w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
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
        let mut w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
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
    fn scrim_dismiss_off_ignores_the_same_gesture() {
        let mut w = build(&view(
            settled(ModalConfig::centered(MAX_WIDTH_LG)).scrim_dismiss(false),
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
    fn the_close_button_fires_on_release_inside_and_hovers() {
        let mut w = build(&view(
            settled(ModalConfig::centered(MAX_WIDTH_LG)).close_button(true),
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
    fn the_drag_handle_dismisses_on_release() {
        let mut w = build(&view(
            settled(ModalConfig::edge(OverlaySide::Bottom)).handle(true),
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
            settled(ModalConfig::centered(MAX_WIDTH_LG)).close_button(true),
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
    fn the_fade_zoom_entrance_ramps_alpha_and_scale_then_settles() {
        let mut w = build(&view(ModalConfig::centered(MAX_WIDTH_LG)));
        let size = layout(&mut w);
        let frame = |w: &mut ModalWidget, ms: f64| {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, ft_ms(ms));
            w.paint(&mut ctx, &mut rec);
            rec
        };
        let first = frame(&mut w, 0.0);
        assert_eq!(first.layers, vec![0.0], "starts transparent");
        assert_eq!(first.rects[0].2.components[3], 0.0, "so does the scrim");
        let mid = frame(&mut w, FADE_ZOOM_MS as f64 / 2.0);
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "mid-fade: {alpha}");
        assert_eq!(mid.transforms.len(), 1, "and a zoom transform");
        let done = frame(&mut w, FADE_ZOOM_MS as f64 * 2.0);
        assert!(
            done.layers.is_empty() && done.transforms.is_empty(),
            "a settled panel composites plainly"
        );
        assert!((w.progress() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn reduce_motion_collapses_the_entrance_to_a_jump() {
        let mut theme = crate::theme().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;
        let mut w = build(&view(ModalConfig::centered(MAX_WIDTH_LG)));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert!((w.progress() - 1.0).abs() < 1e-9, "settled on the spot");
        assert!(rec.layers.is_empty(), "no fade layer");
        assert_eq!(
            rec.rects[0].2,
            theme.scheme().scrim,
            "a full-strength scrim"
        );
    }

    #[test]
    fn the_slide_entrance_moves_the_panel_in_layout() {
        let mut w = build(&view(ModalConfig::edge(OverlaySide::Right)));
        let size = layout(&mut w);
        assert_eq!(
            w.panel_rect().x0,
            WINDOW.width,
            "starts entirely off the right edge"
        );
        let frame = |w: &mut ModalWidget, ms: f64| {
            let mut rec = Recorder::default();
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, ft_ms(ms));
            w.paint(&mut ctx, &mut rec);
            layout(w);
        };
        frame(&mut w, 0.0);
        frame(&mut w, SLIDE_MS as f64 / 2.0);
        let mid = w.panel_rect();
        assert!(
            mid.x0 > WINDOW.width - mid.width() && mid.x0 < WINDOW.width,
            "mid-slide: {}",
            mid.x0
        );
        frame(&mut w, SLIDE_MS as f64 * 2.0);
        assert_eq!(w.panel_rect().x1, WINDOW.width, "flush at rest");
    }

    #[test]
    fn the_slide_entrances_seeding_paint_requests_a_layout() {
        // `AnimationController::advance`'s first truthy call after `forward()`
        // only seeds the clock — `progress` is still 0.0 after it. The
        // continuation has to be requested on that seeding paint too, or a
        // layout-affecting entrance (the sheet/drawer slide) never schedules
        // the frame that would carry it off zero, and the panel sits
        // permanently off-screen under a `ControlFlow::Wait` shell.
        let mut w = build(&view(ModalConfig::edge(OverlaySide::Right)));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, ft_ms(0.0));
        w.paint(&mut ctx, &mut rec);
        assert!(
            w.progress().abs() < 1e-9,
            "the seeding paint leaves progress at zero"
        );
        assert!(
            ctx.needs_layout(),
            "the seeding paint must still schedule the frame that ramps progress off zero"
        );
    }

    #[test]
    fn the_fade_zoom_entrances_seeding_paint_requests_a_frame() {
        // The paint-only counterpart of the slide regression above: FadeZoom is
        // not layout-affecting, so its continuation is a bare frame request.
        // Pinned here so a future change to the shared `advance` branch cannot
        // silently regress it alongside the slide.
        let mut w = build(&view(ModalConfig::centered(MAX_WIDTH_LG)));
        let size = layout(&mut w);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, ft_ms(0.0));
        w.paint(&mut ctx, &mut rec);
        assert!(
            w.progress().abs() < 1e-9,
            "the seeding paint leaves progress at zero"
        );
        assert!(
            ctx.needs_frame(),
            "the seeding paint must still schedule the frame that ramps progress off zero"
        );
        assert!(
            !ctx.needs_layout(),
            "a non-layout-affecting entrance must not also request a layout pass"
        );
    }

    // ---- The staged exit ----------------------------------------------------

    #[test]
    fn a_dismiss_ramps_the_panel_out_and_closes_only_when_it_settles() {
        let (view, closed) = staged(ModalConfig::centered(MAX_WIDTH_LG));
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
        let mid = frame(&mut w, 3000.0 + FADE_ZOOM_MS as f64 / 2.0);
        assert!(w.progress() > 0.0 && w.progress() < 1.0, "{}", w.progress());
        let alpha = mid.layers[0];
        assert!(alpha > 0.0 && alpha < 1.0, "the panel fades out: {alpha}");
        assert!(
            mid.rects[0].2.components[3] < crate::overlay::SCRIM_ALPHA,
            "and the scrim rides the ramp down"
        );
        assert_eq!(closed.get(), 0);

        // Settled: progress at zero, and the close fires exactly once.
        frame(&mut w, 3000.0 + FADE_ZOOM_MS as f64 * 2.0);
        assert_eq!(w.progress(), 0.0);
        assert_eq!(closed.get(), 1);
        assert!(!w.is_exiting());
        assert_eq!(state.dismissed, 0);
    }

    #[test]
    fn the_exit_keeps_asking_for_the_pass_its_own_motion_needs() {
        // The mirror of the two seeding-paint tests above, for the exit ramp: a
        // slide moves the panel's geometry and needs a relayout, a fade/zoom is
        // paint-only and must not ask for one.
        for (config, layout_affecting) in [
            (ModalConfig::edge(OverlaySide::Bottom), true),
            (ModalConfig::centered(MAX_WIDTH_LG), false),
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
        let mut theme = crate::theme().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;
        let (view, closed) = staged(ModalConfig::centered(MAX_WIDTH_LG));
        let mut w = build(&view);
        let size = layout(&mut w);
        let paint = |w: &mut ModalWidget, ms: f64| {
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
        let (view, closed) = staged(ModalConfig::centered(MAX_WIDTH_LG));
        let mut w = build(&view);
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        let first = w.progress();
        frame(&mut w, 3000.0);
        frame(&mut w, 3000.0 + FADE_ZOOM_MS as f64 / 2.0);
        let mid = w.progress();
        assert!(mid < first, "the ramp is running");

        // A second trigger neither restarts the ramp nor fires anything.
        dispatch(&mut w, &mut state, &escape());
        assert_eq!(w.progress(), mid, "the ramp is untouched");
        frame(&mut w, 3000.0 + FADE_ZOOM_MS as f64 * 2.0);
        assert_eq!(closed.get(), 1, "one close for two triggers");
        assert_eq!(state.dismissed, 0);
    }

    #[test]
    fn without_a_close_hook_the_dismissal_stays_unstaged() {
        // A `Stack`-mounted modal that only wired `on_dismiss` has no
        // state-bearing pass to defer into, so it keeps the immediate path
        // rather than leaving an invisible barrier standing.
        let mut w = build(&view(ModalConfig::centered(MAX_WIDTH_LG)));
        layout(&mut w);
        run_ramp(&mut w, 0.0);
        let mut state = Flags::default();
        dispatch(&mut w, &mut state, &escape());
        assert_eq!(state.dismissed, 1);
        assert!(!w.is_exiting());
        assert!((w.progress() - 1.0).abs() < 1e-9, "nothing animates out");
    }

    #[test]
    fn the_barrier_swallows_a_key_the_content_did_not_take() {
        let mut w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
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

    // ---- Drag-to-close and snap points --------------------------------------

    /// A bottom drawer's chrome: content-tall, capped, draggable, handled.
    fn drawer_config() -> ModalConfig {
        ModalConfig::edge(OverlaySide::Bottom)
            .corners(ModalCorners::Top)
            .handle(true)
            .drag(true)
    }

    /// Build an entered, draggable bottom panel plus its close counter.
    fn dragged() -> (ModalWidget, Rc<Cell<u32>>) {
        let (view, closed) = staged(drawer_config());
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
    fn a_flick_toward_the_edge_closes_from_anywhere() {
        // A tall panel, so the flick's own travel stays a small fraction of the
        // extent and the *speed* is what decides.
        let closed = Rc::new(Cell::new(0u32));
        let hook = closed.clone();
        let mut w = build(
            &modal(Block(Size::new(200.0, 400.0)), drawer_config())
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
        // well past `FLING_VELOCITY`, while the panel is still nearly open.
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
    fn catching_a_closing_drawer_cancels_its_dismissal() {
        let (mut w, closed) = dragged();
        let mut state = Flags::default();
        let handle = w.handle_rect().unwrap().center();
        dispatch(&mut w, &mut state, &escape());
        frame(&mut w, 3000.0);
        frame(&mut w, 3000.0 + SLIDE_MS as f64 / 4.0);
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
        // Halfway and fully open, the way a Base UI drawer authors them.
        let (view, closed) = staged(drawer_config());
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
        let (view, _closed) = staged(drawer_config());
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
            "60 logical px of the drawer are on screen"
        );
    }

    #[test]
    fn semantics_is_a_modal_node_labelled_by_its_title() {
        fn logic(_s: &mut Flags) -> ModalView<Flags> {
            modal(
                Block(Size::new(100.0, 50.0)),
                settled(ModalConfig::centered(MAX_WIDTH_LG)).role(ModalRole::AlertDialog),
            )
            .label("Are you sure?")
        }
        let mut root: RenderRoot<Flags, ModalView<Flags>> = RenderRoot::new();
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
        let w = build(&view(settled(ModalConfig::centered(MAX_WIDTH_LG))));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 1);
    }
}
