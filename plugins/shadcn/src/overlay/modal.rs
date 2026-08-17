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
//!   [`ModalConfig::handle`] is on. It is a *button* in v1: press and release on
//!   it dismisses. Drag-to-close (vaul's own gesture) is deferred.
//!
//! plus **Escape**, once the modal holds focus — claimed on every `Down`, the
//! `frust_material::dialog` opt-in, with the same documented gap: there is no
//! auto-focus-on-appear hook in the framework, so a caller must complete one
//! pointer interaction with the modal before Escape does anything.
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
//! and progress snaps to `1.0` on the first paint, one frame after the modal
//! mounts (the pass that sets it is a paint, and the geometry it feeds is a
//! layout) — so a reduced-motion modal appears whole on its second frame rather
//! than animating on its first.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size,
    ThemeTextColor, View, Widget, any, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child, text::FontWeight, visit_children,
};
use frust::{
    AnimationController, CrossAxisAlignment, Curve, EdgeInsets, FlexChild, FlexView,
    NavigatorController, Padding, PopResult, SizedBox, Theme, TransitionSpec, flexible, inflexible,
    text,
};
use kurbo::RoundedRectRadii;

use super::{OverlaySide, finite_or_zero};
use crate::style::{self, ShadcnShadow};
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
/// Progress difference below which an entrance counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

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

/// Flattening tolerance for the panel's border/corner paths.
const PATH_TOLERANCE: f64 = 0.1;
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

    /// Set the accessibility role.
    pub fn role(mut self, role: ModalRole) -> Self {
        self.role = role;
        self
    }
}

/// A view-held, typed dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A modal component that [`show_modal`] can wire a navigator pop into.
///
/// Every modal builder in the catalog implements it by storing the callback in
/// its own `on_dismiss` slot; the trait exists so one push helper serves all
/// five rather than each component re-deriving the same
/// `push_transparent_for_result` call.
pub trait ModalContent<State: 'static>: View<State> + Sized + 'static {
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
}

impl<State: 'static> ModalView<State> {
    /// Label the modal's accessibility node (its title text).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the dismiss callback — a scrim tap, the close X, the handle, or
    /// Escape. [`show_modal`] wires this to `controller.pop()`.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
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
/// its own entrance ([`ModalEntrance`]), so a page transition on top of it would
/// animate the same thing twice.
pub fn show_modal<State, V, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    V: ModalContent<State>,
    B: Fn() -> V + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss_ctrl = controller.clone();
    controller.push_transparent_for_result(
        move || {
            let ctrl = dismiss_ctrl.clone();
            any::<State, _>(build().on_modal_dismiss(Rc::new(move |_state: &mut State| ctrl.pop())))
        },
        TransitionSpec::NONE,
        on_result,
    );
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
    /// The panel rect in the widget's own coordinate space (computed at layout,
    /// hit-tested at event time).
    panel: Rect,
    /// The entrance ramp, and the progress `layout` last used.
    anim: AnimationController,
    progress: f64,
    /// Whether the first paint has seeded the ramp (see the module docs'
    /// `reduce_motion` note).
    started: bool,
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

    /// The entrance ramp's progress: `0.0` off-screen/transparent, `1.0` at
    /// rest.
    pub fn progress(&self) -> f64 {
        self.progress
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

    /// Fire the dismiss callback, if one is wired.
    fn dismiss(&mut self, ctx: &mut EventCtx) {
        if let Some(on_dismiss) = self.on_dismiss.as_mut() {
            on_dismiss(ctx);
        }
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
            panel: Rect::ZERO,
            anim: AnimationController::new(self.config.entrance.duration())
                .with_curve(self.config.entrance.curve()),
            progress: if settled { 1.0 } else { 0.0 },
            started: settled,
            scrim_captured: false,
            scrim_down_outside: false,
            close_hovered: false,
            close_captured: false,
            handle_hovered: false,
            handle_captured: false,
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
        // Closures aren't comparable, so the dismiss adapter is reinstalled
        // unconditionally (cheap — what every interactive widget does).
        element.on_dismiss = self.on_dismiss.as_ref().map(erase_callback);
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
            // The slide: at `progress == 0` the panel sits entirely outside its
            // own edge, at `1.0` it is flush against it.
            let out = thickness * (1.0 - progress);
            let origin = match side {
                OverlaySide::Top => Point::new(0.0, -out),
                OverlaySide::Bottom => Point::new(0.0, area.height - thickness + out),
                OverlaySide::Left => Point::new(-out, 0.0),
                OverlaySide::Right => Point::new(area.width - thickness + out, 0.0),
            };
            let panel = Rect::from_origin_size(origin, size);
            content.set_origin(panel.origin());
            panel
        }
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

        // Advance (or collapse) the entrance.
        if !self.started {
            self.started = true;
            if !reduce_motion {
                self.anim.forward();
            }
        }
        let next = if reduce_motion {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            1.0
        } else if self.anim.is_animating() {
            if self.anim.advance(now) && !self.config.entrance.is_layout_affecting() {
                // Paint-only motion (the fade/zoom): a plain frame request.
                ctx.request_frame();
            }
            self.anim.value_clamped()
        } else {
            self.progress
        };
        if (next - self.progress).abs() > PROGRESS_EPSILON {
            self.progress = next;
            if self.config.entrance.is_layout_affecting() {
                // The slide moves the panel, so the geometry must be recomputed
                // — `request_layout` implies a frame.
                ctx.request_layout();
            } else {
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
        // this widget's own hover claims come after the routing.
        if route_event_single(&mut self.content, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if let InputEvent::Key(key) = event {
            if key.key == Key::Named(NamedKey::Escape) && self.on_dismiss.is_some() {
                self.dismiss(ctx);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let (close, handle) = (self.close_rect(), self.handle_rect());
        match p.phase {
            PointerPhase::Move => {
                if self.close_captured || self.handle_captured || self.scrim_captured {
                    // A captured drag re-asks for its cursor from its own arm.
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
                    ctx.set_cursor(style::ACTIVE_CURSOR);
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
                        self.dismiss(ctx);
                    }
                    return EventResult::Handled;
                }
                if self.handle_captured {
                    self.handle_captured = false;
                    if hit(handle, p.position) {
                        self.dismiss(ctx);
                    }
                    return EventResult::Handled;
                }
                if !self.scrim_captured {
                    return EventResult::Ignored;
                }
                self.scrim_captured = false;
                let released_outside = !self.panel.contains(p.position);
                if self.config.scrim_dismiss && self.scrim_down_outside && released_outside {
                    self.dismiss(ctx);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // A `Cancel` arm never touches app state — flags only.
                self.close_captured = false;
                self.handle_captured = false;
                self.scrim_captured = false;
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
