// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/lists/ — controllers/m3e_dismissible_card_controller.dart,
//   controllers/m3e_dismissible_card_drag_mixin.dart,
//   controllers/m3e_dismissible_card_build_mixin.dart,
//   components/m3e_dismissible_list_widgets.dart,
//   styles/m3e_dismissible_list_style.dart, styles/m3e_list_theme.dart's
//   `M3EListDismissibleTheme`, models/m3e_dismissible_slot.dart (retrieved
//   2026-08-19).
// Ported as a per-item wrapper rather than upstream's slot-owning list, so the
// neighbour-pull/roundness-snap choreography, the frozen-child slot registry,
// and the pull-haptic stream stay unported — see the module docs' Not ported.

//! The M3E **dismissible** wrapper: drag a row horizontally to reveal a
//! per-direction background, release past the commit line to send it away.
//!
//! [`dismissible`] wraps exactly one child and paints no surface of its own —
//! the child owns its container ([`crate::card`], a
//! [`crate::list_item`]`.contained()`, or a row hosted by
//! [`crate::card_list`]), the same division [`crate::card_list`]'s Children
//! section draws. The wrapper contributes the drag, the revealed background
//! band, and the exit animation.
//!
//! # Axis arbitration: a horizontal *intent*, not a horizontal *position*
//!
//! A `Down` captures the pointer and is forwarded to the child, but the drag
//! stays **unresolved** until a `Move` clears [`frust::input::TOUCH_SLOP`]:
//!
//! - past the slop with `|dx| > |dy|` — a horizontal intent. The wrapper takes
//!   the gesture over, sends the child a synthetic `Cancel` (disarming any tap
//!   it armed), and owns every later pass until the release.
//! - past the slop with `|dy| >= |dx|` — a vertical intent. The gesture is
//!   **declined** for its whole remaining life: no later `Move`, however
//!   horizontal, can revive it, and events keep flowing to the child so an
//!   enclosing `ScrollView` can take the scroll the way it takes any other
//!   (it captured on its own `Down` above this widget, so it arbitrates from
//!   outside; the decline is what stops *both* from acting on one gesture).
//!
//! That is the same slop-then-takeover shape [`crate::carousel`] uses, plus the
//! axis comparison a vertical host makes necessary.
//!
//! # Release: threshold, then velocity
//!
//! `progress` is `|offset| / (width · `[`DismissibleView::threshold`]`)`,
//! clamped to `0..=1` — upstream's `_dragProgress`. A release commits when
//! **either** `progress` reaches `1.0` **or** the release speed clears
//! [`DISMISS_FLING_VELOCITY`]; anything else springs back to rest. Below-commit
//! releases never fire [`DismissibleView::on_dismissed`].
//!
//! The velocity arm is a **port addition**: upstream decides on the threshold
//! alone and uses the release velocity only to scale its spring stiffnesses
//! (`speedMul`, which this port keeps — see [`speed_multiplier`]). A fast flick
//! that has not travelled 20% of the row still reads as a dismiss on every
//! platform's own swipe-to-delete, so the threshold-only rule is the one place
//! this port deliberately widens the reference.
//!
//! # Exit: slide out, then collapse
//!
//! A committed dismissal runs upstream's two-stage exit in order: the row
//! **slides** to `±(width + `[`DISMISS_FLY_OVERSHOOT`]`)`, and once that slide
//! is [`DISMISS_COLLAPSE_HANDOFF`] of the way there (or lands first) the row's
//! **height collapses** to zero underneath it. The two overlap exactly as they
//! do upstream (`_startFlyOut`'s 90% listener starting `colCtrl`), so the row
//! never appears to shrink before it has left.
//!
//! **`on_dismissed` fires at the commit, not after the exit** — upstream's own
//! order (`_dismiss` calls `onDismissCallback` right after `_startFlyOut`, not
//! from the collapse's completion listener). It is also the only order
//! reachable here: a state-bearing callback needs an `EventCtx`, which only the
//! event pass carries, and `Up` is the last event a released gesture produces —
//! `frust_core::mark_pending_result_flush` (the framework's deferred-callback
//! seam) is not reachable from a design-system plugin, the same constraint
//! [`crate::selection`] and [`crate::toggle_button`] document for their
//! long-press timers. An app that removes the row inside `on_dismissed` therefore
//! unmounts it mid-exit and sees no animation; one that wants the exit keeps the
//! row mounted for the ~1s the collapse takes (the *kept-mounted* shape
//! `frust_shadcn`'s anchored overlay documents) and removes it after.
//!
//! # Clamp discipline
//!
//! `f64::clamp` panics if `min > max` or either bound is NaN — a hard abort
//! under `panic = "abort"`. Every clamp in this module clamps against the
//! literal `0.0`/`1.0` pair ([`DismissibleView::progress_at`],
//! [`speed_multiplier`], the exit ramps' `value_clamped`), which is trivially
//! ordered and finite; the one derived quantity, the revealed band's width, is
//! ordered with `max(0.0)` rather than a clamp. [`DismissibleView::progress_at`]
//! additionally early-returns on a non-positive denominator, so a row measured
//! at zero width (before the first layout, or under a degenerate threshold)
//! reads `0.0` progress instead of dividing by zero into an instant dismiss.
//! A new clamp here must fit one of those shapes. See
//! [`crate::slider`]'s core for the same rule stated at length.
//!
//! # Hit-testing follows the *unslid* geometry
//!
//! The slide is a paint-only [`frust::authoring::PaintScene::push_transform`],
//! so a mid-drag row still hit-tests where it was laid out — the v1 rule
//! `frust_widgets`' `motion::animated` wrappers established for every
//! compositing-primitive effect. It costs nothing here, since the wrapper owns
//! every pointer pass for the length of a drag anyway.
//!
//! # Not ported
//!
//! * **The slot registry** (`M3EDismissibleSlot`, `_syncSlots`,
//!   `frozenChild`, `capturedHeight`/`capturedWidth`) — upstream's list owns a
//!   parallel slot list so a dismissed row can keep animating after the data
//!   index is gone. This wrapper animates its own live child instead, which is
//!   what makes it composable with any list (`frust::list_view`,
//!   [`crate::card_list`], a plain `Column`) rather than being one.
//! * **The neighbour pull and roundness snap** (`neighbourPull`,
//!   `neighbourReach`, `_roundnessFraction`, `computeRadius`'s drag-adjacent
//!   arms) — every one of them is a *between-rows* effect that only a list
//!   owning all its rows can stage. A wrapper sees one row.
//! * **The detach push** (`_kDetachPush`, `_kDetachPushPixels`) and the
//!   re-engage spring — the past-threshold nudge upstream applies only when no
//!   background is configured.
//! * **`dismissHapticStream`** — the continuous 60ms-throttled pull haptics.
//!   [`DismissibleView::haptic`] fires once, at the commit, which is what
//!   `hapticOnThreshold` does.
//! * **`onTap`/`onLongPress`/`colorBuilder`/`borderRadiusBuilder`** — row
//!   concerns, not the wrapper's: the child owns its own press handling, and
//!   this wrapper forwards every pointer pass to it until a drag is resolved.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::input::{TOUCH_SLOP, VelocityTracker};
use frust::{AnimationController, FrameTime, IconData, IconSource, SpringDesc, Theme};
use kurbo::{Affine, BezPath, Point, Size, Vec2};
use peniko::{Brush, Color};

use crate::interaction::{HapticSignal, MaterialHaptics};
use crate::press::presses;

/// Fraction of the row's width a drag must cover before a release dismisses
/// (`M3EListDismissibleTheme.defaultDismissThreshold`, 0.2).
pub const DISMISS_THRESHOLD: f64 = 0.2;
/// Gap between the swiped row's trailing edge and the revealed background band
/// (`M3EListDismissibleTheme.defaultActionGap`, 8dp).
pub const DISMISS_ACTION_GAP: f64 = 8.0;
/// Corner radius of the revealed background band
/// (`M3EListDismissibleTheme.defaultBackgroundBorderRadius`, 100dp — a pill at
/// any row height).
pub const DISMISS_BACKGROUND_RADIUS: f64 = 100.0;
/// How far past the row's own width a committed dismissal slides
/// (`_startFlyOut`'s `capturedWidth + 80.0`).
pub const DISMISS_FLY_OVERSHOOT: f64 = 80.0;
/// The fraction of the slide after which the height collapse starts underneath
/// it (`_startFlyOut`'s `currentDist / totalDist > 0.9` listener).
pub const DISMISS_COLLAPSE_HANDOFF: f64 = 0.9;
/// Release speed (logical px/s along the drag axis) at or above which a release
/// commits regardless of how far it travelled.
///
/// A **port addition** — upstream commits on the threshold alone (see the
/// [module docs](self)' Release section). The value mirrors this crate's other
/// two fling thresholds ([`crate::overlay::OVERLAY_FLING_VELOCITY`] and the
/// bottom sheet's), an order of magnitude above `frust::input::FLING_STOP`'s
/// 30 px/s "this fling is over" floor and well under a full-screen swipe.
pub const DISMISS_FLING_VELOCITY: f64 = 400.0;
/// Side length of a background band's icon, in logical px (Flutter's default
/// `Icon` size, which upstream's bare background `Icon(...)` inherits).
pub const DISMISS_ICON_SIZE: f64 = 24.0;

/// Progress below which a background icon stays fully transparent, and the
/// scale it holds there (`_buildActiveBackground`'s `progress < 0.3` arm).
const ICON_FADE_START: f64 = 0.3;
/// The icon's scale at [`ICON_FADE_START`], ramping to `1.0` at full progress
/// (`0.8 + ((progress - 0.3) / 0.7) * 0.2`).
const ICON_MIN_SCALE: f64 = 0.8;
/// Multiplier on `progress` for the band's own fade-in
/// (`Opacity(opacity: (_dragProgress * 3.0).clamp(0, 1))`).
const BAND_FADE_GAIN: f64 = 3.0;

/// Fallback background fill (a theme resolves this from
/// `colors.error_container`) — the M3 baseline light `errorContainer` tone.
const ERROR_CONTAINER: Color = Color::from_rgb8(0xF9, 0xDE, 0xDC);
/// Fallback background icon color (a theme resolves this from
/// `colors.on_error_container`).
const ON_ERROR_CONTAINER: Color = Color::from_rgb8(0x41, 0x0E, 0x0B);

/// Base stiffness of the slide-out spring, before [`speed_multiplier`]
/// (`_startFlyOut`'s `stiffness: 400 * speedMul`, damping `0.8`).
const SLIDE_STIFFNESS: f64 = 400.0;
/// Damping ratio of the slide-out spring (`_startFlyOut`'s `damping: 0.8`).
const SLIDE_DAMPING: f64 = 0.8;
/// Base stiffness of the spring-back leg (`_springBack`'s
/// `stiffness: 380 * speedMul`, damping `0.6`).
const SPRING_BACK_STIFFNESS: f64 = 380.0;
/// Damping ratio of the spring-back leg (`_springBack`'s `damping: 0.6`).
const SPRING_BACK_DAMPING: f64 = 0.6;
/// Base stiffness of the height collapse
/// (`M3EListDismissibleTheme.defaultCollapseSpeed`, 50, fed to
/// `_kSpatialSpringBack.copyWith(stiffness: collapseSpeed * speedMul)`).
const COLLAPSE_STIFFNESS: f64 = 50.0;
/// Damping ratio of the height collapse (`_kSpatialSpringBack`'s `0.8`).
const COLLAPSE_DAMPING: f64 = 0.8;

/// Nominal period seeding an exit [`AnimationController`]'s clock; every leg
/// here is spring-driven via `fling`, so this backs construction only.
const EXIT_PERIOD: std::time::Duration = std::time::Duration::from_millis(300);
/// Launch velocity for a `0 → 1` exit leg. Sub-visible on purpose: only the
/// sign selects the target, and every leg here runs forward (the same
/// sign-only convention [`crate::switch`]'s `RELEASE_VELOCITY` documents).
const EXIT_LAUNCH_VELOCITY: f64 = 1e-3;

/// The spring-stiffness multiplier a release velocity earns
/// (`handleDragEnd`'s `(1.0 + velocity / 1000.0).clamp(1.0, 4.0)`): a faster
/// flick sends the row away proportionally faster.
///
/// `velocity` is in logical px/s; its sign is irrelevant.
pub fn speed_multiplier(velocity: f64) -> f64 {
    if !velocity.is_finite() {
        return 1.0;
    }
    (1.0 + velocity.abs() / 1000.0).clamp(1.0, 4.0)
}

/// The direction a row was swiped (Flutter's `DismissDirection`, narrowed to
/// the two horizontal arms upstream uses).
///
/// `Start`/`End` resolve **left-to-right**: `StartToEnd` is a rightward swipe.
/// The framework threads no directionality into a widget's event pass, so this
/// port does not mirror the pair under RTL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DismissDirection {
    /// Swiped from the start edge toward the end one — rightward, in LTR
    /// (`DismissDirection.startToEnd`).
    StartToEnd,
    /// Swiped from the end edge toward the start one — leftward, in LTR
    /// (`DismissDirection.endToStart`).
    EndToStart,
}

impl DismissDirection {
    /// The direction a signed drag offset points, or `None` at rest.
    fn of(offset: f64) -> Option<Self> {
        if offset > 0.0 {
            Some(Self::StartToEnd)
        } else if offset < 0.0 {
            Some(Self::EndToStart)
        } else {
            None
        }
    }
}

/// The band revealed behind a row as it is swiped: a fill and an optional
/// centered icon (upstream's `background`/`secondaryBackground` widgets,
/// narrowed to the fill+icon shape they are used for).
#[derive(Clone, Copy, Debug, Default)]
pub struct DismissBackground {
    color: Option<Color>,
    icon: Option<IconSource>,
    icon_color: Option<Color>,
    radius: Option<f64>,
}

/// An empty background band — chain [`DismissBackground::color`] /
/// [`DismissBackground::icon`] onto it. Unset, the fill resolves from
/// `colors.error_container` and the icon from `colors.on_error_container`.
pub fn dismiss_background() -> DismissBackground {
    DismissBackground::default()
}

impl DismissBackground {
    /// Replace the band's fill, which otherwise resolves from
    /// `colors.error_container`.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Center this icon in the band; it fades and scales in with the drag.
    pub fn icon(mut self, icon: IconSource) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Replace the icon's color, which otherwise resolves from
    /// `colors.on_error_container`.
    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// Override the band's corner radius. Defaults to
    /// [`DISMISS_BACKGROUND_RADIUS`].
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = Some(radius);
        self
    }
}

/// Whether two optional icon sources name the same glyph — [`IconSource`] is
/// `Copy` but not `PartialEq`, and its `(d, design)` pair is its identity.
fn same_icon(a: Option<IconSource>, b: Option<IconSource>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.d == b.d && a.design == b.design,
        _ => false,
    }
}

/// Resolve a background's icon into its `(path, design size)` pair once, so
/// paint never re-parses the SVG data (the caching [`crate::switch`] does).
fn icon_geometry(background: Option<&DismissBackground>) -> Option<(BezPath, f64)> {
    background
        .and_then(|b| b.icon)
        .map(|source| IconData::from(source).resolve())
}

// ---- The exit choreography ------------------------------------------------

/// Which stage of the dismissal is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    /// Nothing animating — at rest, or mid-drag under the finger.
    Rest,
    /// Released below the commit line: the offset springs back to zero.
    SpringBack,
    /// Committed: the row slides out and (past the handoff) collapses.
    Exit,
}

/// How far a resolved gesture got before the pointer went down.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    /// No pointer down.
    None,
    /// Down, axis not yet resolved — every pass still reaches the child.
    Pending { start: Point },
    /// Resolved horizontal: this widget owns the gesture.
    Dragging { start_x: f64 },
    /// Resolved vertical: this gesture can never dismiss.
    Declined,
}

// ---- The view -------------------------------------------------------------

/// A view-held, typed dismissal callback (erased on build).
type OnDismissed<State> = Rc<dyn Fn(&mut State, DismissDirection)>;

/// A declarative swipe-to-dismiss wrapper. See the [module docs](self).
pub struct DismissibleView<State: 'static> {
    child: AnyView<State>,
    background: Option<DismissBackground>,
    secondary_background: Option<DismissBackground>,
    threshold: f64,
    action_gap: f64,
    only: Option<DismissDirection>,
    enabled: bool,
    haptic: HapticSignal,
    on_dismissed: Option<OnDismissed<State>>,
}

/// Wrap `child` in a swipe-to-dismiss gesture. Configure at least one of
/// [`background`](DismissibleView::background) /
/// [`secondary_background`](DismissibleView::secondary_background) to reveal
/// anything behind it, and handle
/// [`on_dismissed`](DismissibleView::on_dismissed) to remove the row.
pub fn dismissible<State: 'static, V: View<State>>(child: V) -> DismissibleView<State> {
    DismissibleView {
        child: any(child),
        background: None,
        secondary_background: None,
        threshold: DISMISS_THRESHOLD,
        action_gap: DISMISS_ACTION_GAP,
        only: None,
        enabled: true,
        haptic: HapticSignal::None,
        on_dismissed: None,
    }
}

impl<State: 'static> DismissibleView<State> {
    /// The band revealed by a [`DismissDirection::StartToEnd`] (rightward)
    /// swipe — and by a leftward one too, unless
    /// [`secondary_background`](Self::secondary_background) overrides it
    /// (upstream's `secondaryBackground ?? background` fallback).
    pub fn background(mut self, background: DismissBackground) -> Self {
        self.background = Some(background);
        self
    }

    /// The band revealed by a [`DismissDirection::EndToStart`] (leftward)
    /// swipe.
    pub fn secondary_background(mut self, background: DismissBackground) -> Self {
        self.secondary_background = Some(background);
        self
    }

    /// The fraction of the row's width a drag must cover to commit on release.
    /// Defaults to [`DISMISS_THRESHOLD`]. A non-positive value makes the
    /// position arm unreachable (see the [module docs](self)' Clamp
    /// discipline), leaving the velocity arm to decide.
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    /// The gap between the row's trailing edge and the revealed band. Defaults
    /// to [`DISMISS_ACTION_GAP`].
    pub fn action_gap(mut self, gap: f64) -> Self {
        self.action_gap = gap;
        self
    }

    /// Allow dismissal in this direction only; both are allowed by default. A
    /// drag the other way never resolves horizontally at all, so it stays
    /// available to the child and to an enclosing scroll surface.
    pub fn only(mut self, direction: DismissDirection) -> Self {
        self.only = Some(direction);
        self
    }

    /// Gate the whole gesture. `false` forwards every pointer pass to the child
    /// untouched — no drag, no background, no dismissal. Defaults to `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The haptic fired immediately before [`on_dismissed`](Self::on_dismissed)
    /// on a committed release (`M3EDismissibleListStyle.hapticOnThreshold`).
    /// Defaults to [`HapticSignal::None`].
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Fire `on_dismissed` with the swipe's direction when a release commits.
    /// Without it the row still drags and springs back, but never dismisses.
    ///
    /// The callback runs at the commit, *before* the exit animation — see the
    /// [module docs](self)' Exit section for why, and what an app that wants to
    /// watch the exit does instead.
    pub fn on_dismissed<F: Fn(&mut State, DismissDirection) + 'static>(
        mut self,
        on_dismissed: F,
    ) -> Self {
        self.on_dismissed = Some(Rc::new(on_dismissed));
        self
    }
}

// ---- The widget -----------------------------------------------------------

/// The retained widget for a [`DismissibleView`].
pub struct DismissibleWidget {
    child: ChildPod,
    background: Option<DismissBackground>,
    secondary_background: Option<DismissBackground>,
    /// `background`'s resolved icon geometry, cached across paints.
    background_icon: Option<(BezPath, f64)>,
    /// `secondary_background`'s resolved icon geometry.
    secondary_icon: Option<(BezPath, f64)>,
    threshold: f64,
    action_gap: f64,
    only: Option<DismissDirection>,
    enabled: bool,
    haptic: HapticSignal,
    on_dismissed: Option<ErasedArgCallback<DismissDirection>>,

    /// The laid-out row width, and the child's own (uncollapsed) height.
    width: f64,
    child_height: f64,
    /// The row's signed horizontal displacement, in logical px.
    offset: f64,
    /// `0.0` full height, `1.0` fully collapsed.
    collapse: f64,
    /// The direction the exit is playing out in, retained for the background.
    exit_direction: Option<DismissDirection>,

    gesture: Gesture,
    motion: Motion,
    tracker: VelocityTracker,
    /// The offset the current spring/slide leg started from, and (for a slide)
    /// where it ends.
    leg_from: f64,
    leg_to: f64,
    ramp: AnimationController,
    collapse_ramp: AnimationController,
    collapse_started: bool,
    /// The release's [`speed_multiplier`], held from the commit until the
    /// collapse leg it also scales actually starts.
    collapse_multiplier: f64,
    /// The last painted frame time — the event pass's only clock source, the
    /// seam [`crate::carousel`] uses for the same reason.
    last_frame_time: FrameTime,
}

/// A spring with `base` stiffness scaled by a release's [`speed_multiplier`].
fn scaled_spring(base: f64, damping_ratio: f64, multiplier: f64) -> SpringDesc {
    SpringDesc {
        mass: 1.0,
        stiffness: base * multiplier,
        damping_ratio,
    }
}

/// The resolved band fill. Themed: `colors.error_container`.
fn resolve_band(theme: Option<&Theme>, background: &DismissBackground) -> Color {
    if let Some(color) = background.color {
        return color;
    }
    match theme {
        Some(theme) => theme.scheme().error_container,
        None => ERROR_CONTAINER,
    }
}

/// The resolved band icon color. Themed: `colors.on_error_container`.
fn resolve_band_icon(theme: Option<&Theme>, background: &DismissBackground) -> Color {
    if let Some(color) = background.icon_color {
        return color;
    }
    match theme {
        Some(theme) => theme.scheme().on_error_container,
        None => ON_ERROR_CONTAINER,
    }
}

/// Return `color` with its alpha channel replaced by `alpha` (the same helper
/// [`crate::card_list`] carries).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

impl<State: 'static> View<State> for DismissibleView<State> {
    type Element = DismissibleWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DismissibleWidget {
        DismissibleWidget {
            child: frust::authoring::build_child(&self.child, ctx),
            background: self.background,
            secondary_background: self.secondary_background,
            background_icon: icon_geometry(self.background.as_ref()),
            secondary_icon: icon_geometry(self.secondary_background.as_ref()),
            threshold: self.threshold,
            action_gap: self.action_gap,
            only: self.only,
            enabled: self.enabled,
            haptic: self.haptic,
            on_dismissed: self
                .on_dismissed
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
            width: 0.0,
            child_height: 0.0,
            offset: 0.0,
            collapse: 0.0,
            exit_direction: None,
            gesture: Gesture::None,
            motion: Motion::Rest,
            tracker: VelocityTracker::new(),
            leg_from: 0.0,
            leg_to: 0.0,
            ramp: AnimationController::new(EXIT_PERIOD),
            collapse_ramp: AnimationController::new(EXIT_PERIOD),
            collapse_started: false,
            collapse_multiplier: 1.0,
            last_frame_time: FrameTime::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DismissibleWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);

        if !same_icon(
            element.background.and_then(|b| b.icon),
            self.background.and_then(|b| b.icon),
        ) {
            element.background_icon = icon_geometry(self.background.as_ref());
            flags |= ChangeFlags::PAINT;
        }
        if !same_icon(
            element.secondary_background.and_then(|b| b.icon),
            self.secondary_background.and_then(|b| b.icon),
        ) {
            element.secondary_icon = icon_geometry(self.secondary_background.as_ref());
            flags |= ChangeFlags::PAINT;
        }
        element.background = self.background;
        element.secondary_background = self.secondary_background;
        element.threshold = self.threshold;
        element.action_gap = self.action_gap;
        element.only = self.only;
        element.haptic = self.haptic;
        element.on_dismissed = self
            .on_dismissed
            .as_ref()
            .map(frust::authoring::erase_callback_arg);

        if element.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // Losing the gesture mid-drag must not leave the row displaced
                // or a resolved drag standing.
                element.gesture = Gesture::None;
                element.motion = Motion::Rest;
                element.offset = 0.0;
                element.exit_direction = None;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut DismissibleWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl DismissibleWidget {
    /// The commit progress a signed `offset` represents: `|offset| / (width ·
    /// threshold)`, clamped to `0..=1` (`_dragProgress`).
    ///
    /// Returns `0.0` unless the denominator is finite and strictly positive — a
    /// row that has not been laid out yet, or a non-positive
    /// [`DismissibleView::threshold`] — rather than dividing into an infinity
    /// (or a `NaN`) that would clamp to an instant dismiss.
    fn progress_at(&self, offset: f64) -> f64 {
        let denominator = self.width * self.threshold;
        if !denominator.is_finite() || denominator <= 0.0 || !offset.is_finite() {
            return 0.0;
        }
        (offset.abs() / denominator).clamp(0.0, 1.0)
    }

    /// Whether a signed drag delta points in an allowed direction.
    fn allows(&self, dx: f64) -> bool {
        match (self.only, DismissDirection::of(dx)) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(allowed), Some(actual)) => allowed == actual,
        }
    }

    /// The band configured for `direction` (`secondaryBackground ?? background`
    /// for a leftward swipe), with its resolved icon geometry.
    fn band(
        &self,
        direction: DismissDirection,
    ) -> Option<(&DismissBackground, Option<&(BezPath, f64)>)> {
        match direction {
            DismissDirection::StartToEnd => self
                .background
                .as_ref()
                .map(|b| (b, self.background_icon.as_ref())),
            DismissDirection::EndToStart => match self.secondary_background.as_ref() {
                Some(b) => Some((b, self.secondary_icon.as_ref())),
                None => self
                    .background
                    .as_ref()
                    .map(|b| (b, self.background_icon.as_ref())),
            },
        }
    }

    /// The last painted frame time in milliseconds — the event pass's timestamp
    /// source for velocity tracking.
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Forward a synthesized `Cancel` to the child a drag has taken the gesture
    /// away from (the shape [`crate::carousel`]'s `cancel_item` takes).
    fn cancel_child(&mut self, ctx: &mut EventCtx, position: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position,
            button: PointerButton::Primary,
        });
        self.child.event_child(ctx, &cancel);
        self.child.set_active(false);
    }

    /// Start the spring-back leg from the current offset.
    fn start_spring_back(&mut self, multiplier: f64) {
        self.leg_from = self.offset;
        self.leg_to = 0.0;
        self.motion = Motion::SpringBack;
        self.ramp = AnimationController::new(EXIT_PERIOD);
        self.ramp.fling(
            EXIT_LAUNCH_VELOCITY,
            scaled_spring(SPRING_BACK_STIFFNESS, SPRING_BACK_DAMPING, multiplier),
        );
    }

    /// Start the committed exit: slide out, then collapse.
    fn start_exit(&mut self, direction: DismissDirection, multiplier: f64) {
        let sign = match direction {
            DismissDirection::StartToEnd => 1.0,
            DismissDirection::EndToStart => -1.0,
        };
        self.exit_direction = Some(direction);
        self.leg_from = self.offset;
        self.leg_to = sign * (self.width + DISMISS_FLY_OVERSHOOT);
        self.motion = Motion::Exit;
        self.collapse_started = false;
        self.collapse = 0.0;
        self.ramp = AnimationController::new(EXIT_PERIOD);
        self.ramp.fling(
            EXIT_LAUNCH_VELOCITY,
            scaled_spring(SLIDE_STIFFNESS, SLIDE_DAMPING, multiplier),
        );
        self.collapse_ramp = AnimationController::new(EXIT_PERIOD);
        self.collapse_multiplier = multiplier;
    }

    /// Advance whatever motion is running; returns
    /// `(needs_frame, needs_layout)`.
    fn advance(&mut self, now: FrameTime) -> (bool, bool) {
        match self.motion {
            Motion::Rest => (false, false),
            Motion::SpringBack => {
                let animating = self.ramp.advance(now);
                let t = self.ramp.value_clamped();
                self.offset = self.leg_from + (self.leg_to - self.leg_from) * t;
                if !animating {
                    self.offset = self.leg_to;
                    self.motion = Motion::Rest;
                }
                (animating, false)
            }
            Motion::Exit => {
                let sliding = self.ramp.advance(now);
                let t = self.ramp.value_clamped();
                self.offset = self.leg_from + (self.leg_to - self.leg_from) * t;
                // Upstream hands off at 90% of the slide distance (or on the
                // slide landing first), so the collapse starts *under* a row
                // that has already left rather than after it.
                if !self.collapse_started && (t >= DISMISS_COLLAPSE_HANDOFF || !sliding) {
                    self.collapse_started = true;
                    self.collapse_ramp.fling(
                        EXIT_LAUNCH_VELOCITY,
                        scaled_spring(
                            COLLAPSE_STIFFNESS,
                            COLLAPSE_DAMPING,
                            self.collapse_multiplier,
                        ),
                    );
                }
                let collapsing = self.collapse_started && self.collapse_ramp.advance(now);
                if self.collapse_started {
                    self.collapse = self.collapse_ramp.value_clamped();
                }
                if !sliding && !collapsing && self.collapse_started {
                    self.collapse = 1.0;
                    self.motion = Motion::Rest;
                }
                // The collapse is a *height* change, computed in `layout`, so
                // it needs a relayout request and not merely another frame —
                // the layout-skip rule `crate::expandable_list` documents.
                (sliding || collapsing, self.collapse_started)
            }
        }
    }

    /// The event body, parameterised on an explicit timestamp so the velocity
    /// math is deterministic in tests; [`Widget::event`] supplies the real
    /// clock.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        // A broadcast is not user input: it reaches the child ahead of every
        // gesture branch and is never consumed.
        if event.is_broadcast() || !self.enabled || self.motion == Motion::Exit {
            return frust::authoring::route_event_single(&mut self.child, ctx, event);
        }
        let InputEvent::Pointer(p) = event else {
            // Key/Ime are focus-routed; this wrapper has no key handling.
            return frust::authoring::route_event_single(&mut self.child, ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    // A secondary press is a context gesture: it reaches the
                    // child but starts no drag.
                    return frust::authoring::route_event_single(&mut self.child, ctx, event);
                }
                // A press during a spring-back stops it and re-seats the row,
                // upstream's `handleDragStart` (`_dragOffset = 0.0`) — the
                // alternative leaves a stale displacement painted for the
                // frames between this `Down` and the axis resolving.
                self.motion = Motion::Rest;
                self.offset = 0.0;
                self.gesture = Gesture::Pending { start: p.position };
                self.tracker.clear();
                self.tracker.record(t_ms, p.position.x);
                ctx.capture_pointer();
                frust::authoring::route_event_single(&mut self.child, ctx, event);
                EventResult::Handled
            }
            PointerPhase::Move => match self.gesture {
                // No press of ours, or one already resolved vertically: the
                // child (and the scroll surface above it) own the pass.
                Gesture::None | Gesture::Declined => {
                    frust::authoring::route_event_single(&mut self.child, ctx, event)
                }
                Gesture::Pending { start } => {
                    self.tracker.record(t_ms, p.position.x);
                    let dx = p.position.x - start.x;
                    let dy = p.position.y - start.y;
                    if dy.abs() > TOUCH_SLOP && dy.abs() >= dx.abs() {
                        // Vertical intent: decline for the rest of the gesture
                        // so an enclosing scroll surface owns it outright.
                        self.gesture = Gesture::Declined;
                        return frust::authoring::route_event_single(&mut self.child, ctx, event);
                    }
                    if dx.abs() > TOUCH_SLOP && dx.abs() > dy.abs() && self.allows(dx) {
                        self.gesture = Gesture::Dragging { start_x: start.x };
                        self.cancel_child(ctx, p.position);
                        self.offset = dx;
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    frust::authoring::route_event_single(&mut self.child, ctx, event)
                }
                Gesture::Dragging { start_x } => {
                    self.tracker.record(t_ms, p.position.x);
                    let dx = p.position.x - start_x;
                    let clamped = match self.only {
                        None => dx,
                        Some(DismissDirection::StartToEnd) => dx.max(0.0),
                        Some(DismissDirection::EndToStart) => dx.min(0.0),
                    };
                    if clamped != self.offset {
                        self.offset = clamped;
                        ctx.request_redraw();
                    }
                    EventResult::Handled
                }
            },
            PointerPhase::Up => {
                let Gesture::Dragging { .. } = self.gesture else {
                    // Unresolved, declined, or never armed here (a secondary
                    // press): the child owns the release, and forwarding is
                    // also what releases the capture it may have recorded.
                    self.gesture = Gesture::None;
                    return frust::authoring::route_event_single(&mut self.child, ctx, event);
                };
                self.gesture = Gesture::None;
                self.tracker.record(t_ms, p.position.x);
                let velocity = self.tracker.velocity();
                let multiplier = speed_multiplier(velocity);
                let past_threshold = self.progress_at(self.offset) >= 1.0;
                let flung = velocity.abs() >= DISMISS_FLING_VELOCITY;
                // A flick decides by *its own* sign; a slow drag by where the
                // row ended up. Either way the direction must be an allowed one.
                let direction = if flung {
                    DismissDirection::of(velocity)
                } else {
                    DismissDirection::of(self.offset)
                };
                match direction.filter(|_| past_threshold || flung) {
                    Some(direction)
                        if self.only.is_none_or(|allowed| allowed == direction)
                            && self.on_dismissed.is_some() =>
                    {
                        if self.haptic != HapticSignal::None {
                            MaterialHaptics::fire(self.haptic);
                        }
                        if let Some(on_dismissed) = self.on_dismissed.as_mut() {
                            (on_dismissed)(ctx, direction);
                        }
                        self.start_exit(direction, multiplier);
                    }
                    _ => self.start_spring_back(multiplier),
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                let resolved = self.gesture;
                self.gesture = Gesture::None;
                match resolved {
                    Gesture::Dragging { .. } => {
                        // A `Cancel` arm only clears internal flags and starts a
                        // purely visual leg — never state, never a callback.
                        self.start_spring_back(1.0);
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    // Everything else still reaches the child, which is also
                    // what releases any capture it recorded on the `Down`.
                    _ => frust::authoring::route_event_single(&mut self.child, ctx, event),
                }
            }
        }
    }

    /// Paint the revealed band for `direction` at the current offset.
    fn paint_band(
        &self,
        scene: &mut dyn PaintScene,
        theme: Option<&Theme>,
        origin: Point,
        height: f64,
        direction: DismissDirection,
    ) {
        let Some((background, icon)) = self.band(direction) else {
            return;
        };
        let revealed = self.offset.abs();
        let band_width = (revealed - self.action_gap).max(0.0);
        if band_width <= 0.0 || height <= 0.0 {
            return;
        }
        let progress = self.progress_at(self.offset);
        let band_alpha = (progress * BAND_FADE_GAIN).clamp(0.0, 1.0) as f32;
        if band_alpha <= 0.0 {
            return;
        }
        let band_x = match direction {
            DismissDirection::StartToEnd => origin.x,
            DismissDirection::EndToStart => origin.x + self.width - band_width,
        };
        let band_origin = Point::new(band_x, origin.y);
        let radius = background.radius.unwrap_or(DISMISS_BACKGROUND_RADIUS);
        scene.fill_rounded_rect(
            band_origin,
            Size::new(band_width, height),
            radius,
            with_alpha(resolve_band(theme, background), band_alpha),
        );

        let Some((path, design)) = icon else {
            return;
        };
        if *design <= 0.0 {
            return;
        }
        // The icon fades in only after the drag is 30% of the way to the commit
        // line, scaling from 0.8 to 1.0 across the rest.
        let ramp = ((progress - ICON_FADE_START) / (1.0 - ICON_FADE_START)).clamp(0.0, 1.0);
        let icon_alpha = (ramp * band_alpha as f64) as f32;
        if icon_alpha <= 0.0 {
            return;
        }
        let icon_scale = ICON_MIN_SCALE + (1.0 - ICON_MIN_SCALE) * ramp;
        let extent = DISMISS_ICON_SIZE * icon_scale;
        let center = Point::new(
            band_origin.x + band_width / 2.0,
            band_origin.y + height / 2.0,
        );
        let transform = Affine::translate(Vec2::new(center.x, center.y))
            * Affine::scale(extent / design)
            * Affine::translate(Vec2::new(-design / 2.0, -design / 2.0));
        scene.fill_path(
            Point::ZERO,
            &(transform * path.clone()),
            &Brush::Solid(with_alpha(resolve_band_icon(theme, background), icon_alpha)),
        );
    }
}

impl Widget for DismissibleWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        self.width = width;
        let child_bc =
            BoxConstraints::new(Size::new(width, 0.0), Size::new(width, bc.max().height));
        let child = self.child.layout_child(ctx, &child_bc);
        self.child_height = child.height;
        self.child.set_origin(Point::ORIGIN);
        // The collapse shrinks the *reported* height only; the child stays laid
        // out at its own, and paint clips it — the same content-size-independent
        // shape `crate::expandable_list`'s reveal takes.
        let height = (child.height * (1.0 - self.collapse)).max(0.0);
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every theme read happens before `ctx` is taken mutably below
        // (`paint_child`) — the ordering `card_list`/`card` share.
        let theme = Theme::from_paint_ctx(ctx);
        self.last_frame_time = ctx.frame_time();
        let (needs_frame, needs_layout) = self.advance(ctx.frame_time());

        let origin = ctx.origin();
        let height = (self.child_height * (1.0 - self.collapse)).max(0.0);
        if height <= 0.0 {
            if needs_layout {
                ctx.request_layout();
            } else if needs_frame {
                ctx.request_frame();
            }
            return;
        }

        // The band belongs to whichever direction the row is displaced in — the
        // exit keeps painting its own after the offset has left the row behind.
        if let Some(direction) = self
            .exit_direction
            .filter(|_| self.motion == Motion::Exit)
            .or_else(|| DismissDirection::of(self.offset))
        {
            self.paint_band(scene, theme, origin, height, direction);
        }

        // A collapsing row must not overflow the height it reports, and a slid
        // one must not paint outside its own column.
        scene.push_clip(origin, Size::new(self.width, height));
        scene.push_transform(Affine::translate(Vec2::new(self.offset, 0.0)));
        self.child.paint_child(ctx, scene);
        scene.pop_transform();
        scene.pop_clip();

        if needs_layout {
            ctx.request_layout();
        } else if needs_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t_ms = self.event_time_ms();
        self.event_at(ctx, event, t_ms)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A transparent wrapper: the row's own node is the one worth
        // publishing, so this contributes none of its own and forwards.
        self.child.semantics_child(ctx);
    }

    frust::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use std::any::Any;

    const WIDTH: f64 = 300.0;
    const ROW_HEIGHT: f64 = 60.0;
    /// The commit distance at the default threshold: 20% of 300dp.
    const COMMIT_PX: f64 = WIDTH * DISMISS_THRESHOLD;

    fn build<S: 'static>(view: &DismissibleView<S>) -> DismissibleWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut DismissibleWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut lctx,
            &BoxConstraints::new(Size::new(WIDTH, 0.0), Size::new(WIDTH, f64::INFINITY)),
        )
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        clips: Vec<(Point, Size)>,
        transforms: Vec<Affine>,
        paths: Vec<BezPath>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {}
        fn fill_path(&mut self, _origin: Point, path: &BezPath, _brush: &Brush) {
            self.paths.push(path.clone());
        }
    }

    /// Paint at `t_ms`, returning `(recorder, needs_frame, needs_layout)`.
    fn paint_at(
        w: &mut DismissibleWidget,
        t_ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(
            Point::ZERO,
            Size::new(WIDTH, ROW_HEIGHT),
            FrameTime::from_nanos(t_ms * 1_000_000),
        );
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    /// Dispatch a pointer event at an explicit millisecond timestamp.
    fn at(
        w: &mut DismissibleWidget,
        state: &mut dyn Any,
        phase: PointerPhase,
        pos: Point,
        t_ms: f64,
    ) -> EventResult {
        let event = InputEvent::Pointer(PointerEvent {
            phase,
            position: pos,
            button: PointerButton::Primary,
        });
        let mut ctx = EventCtx::new(state, Point::ZERO, Size::new(WIDTH, ROW_HEIGHT));
        w.event_at(&mut ctx, &event, t_ms)
    }

    /// A slow horizontal drag to `dx`, ending with the pointer still down.
    /// Steps are 100ms apart, so the tracked velocity stays well under
    /// [`DISMISS_FLING_VELOCITY`].
    fn slow_drag(w: &mut DismissibleWidget, state: &mut dyn Any, dx: f64) -> f64 {
        let mut t = 0.0;
        at(w, state, PointerPhase::Down, Point::new(150.0, 30.0), t);
        let steps = 6;
        for step in 1..=steps {
            t += 100.0;
            let x = 150.0 + dx * (step as f64 / steps as f64);
            at(w, state, PointerPhase::Move, Point::new(x, 30.0), t);
        }
        t
    }

    fn view() -> DismissibleView<u32> {
        dismissible(frust::SizedBox::<u32>(Some(WIDTH), Some(ROW_HEIGHT)))
    }

    /// A recording view: `on_dismissed` pushes `1` for start→end, `2` for
    /// end→start, so a test reads both the count and the direction.
    fn recording() -> DismissibleView<Vec<u32>> {
        dismissible(frust::SizedBox::<Vec<u32>>(Some(WIDTH), Some(ROW_HEIGHT))).on_dismissed(
            |s: &mut Vec<u32>, direction| {
                s.push(match direction {
                    DismissDirection::StartToEnd => 1,
                    DismissDirection::EndToStart => 2,
                })
            },
        )
    }

    // ---- Release decision: threshold ---------------------------------------

    #[test]
    fn progress_is_the_offset_over_the_commit_distance() {
        let mut w = build(&view());
        layout(&mut w);
        assert_eq!(w.progress_at(0.0), 0.0);
        assert!((w.progress_at(COMMIT_PX / 2.0) - 0.5).abs() < 1e-9);
        assert_eq!(w.progress_at(COMMIT_PX), 1.0);
        assert_eq!(
            w.progress_at(-COMMIT_PX),
            1.0,
            "symmetric in both directions"
        );
        assert_eq!(w.progress_at(WIDTH * 10.0), 1.0, "clamped at 1");
    }

    #[test]
    fn progress_is_zero_before_the_first_layout_rather_than_dividing_by_zero() {
        let w = build(&view());
        assert_eq!(w.width, 0.0);
        assert_eq!(w.progress_at(500.0), 0.0);
    }

    #[test]
    fn a_non_positive_threshold_leaves_the_position_arm_unreachable() {
        let mut w = build(&view().threshold(0.0));
        layout(&mut w);
        assert_eq!(w.progress_at(WIDTH), 0.0);
    }

    #[test]
    fn a_slow_release_past_the_threshold_commits_rightward() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, COMMIT_PX + 10.0);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + COMMIT_PX + 10.0, 30.0),
            t,
        );
        assert_eq!(state, vec![1], "start→end reported once");
        assert_eq!(w.motion, Motion::Exit);
        assert_eq!(w.exit_direction, Some(DismissDirection::StartToEnd));
    }

    #[test]
    fn a_slow_release_past_the_threshold_commits_leftward() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, -(COMMIT_PX + 10.0));
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 - COMMIT_PX - 10.0, 30.0),
            t,
        );
        assert_eq!(state, vec![2], "end→start reported once");
        assert_eq!(w.exit_direction, Some(DismissDirection::EndToStart));
    }

    #[test]
    fn a_press_during_a_spring_back_stops_it_and_reseats_the_row() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let short = COMMIT_PX / 2.0;
        let t = slow_drag(&mut w, &mut state, short);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + short, 30.0),
            t,
        );
        // One frame of spring-back, then a fresh press mid-flight.
        paint_at(&mut w, 0, None);
        paint_at(&mut w, 16, None);
        assert_eq!(w.motion, Motion::SpringBack);
        at(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(150.0, 30.0),
            t + 32.0,
        );
        assert_eq!(w.motion, Motion::Rest, "the spring-back is stopped");
        assert_eq!(w.offset, 0.0, "and the row is reseated, never left stale");
        assert!(matches!(w.gesture, Gesture::Pending { .. }));
    }

    #[test]
    fn a_slow_release_short_of_the_threshold_springs_back() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let short = COMMIT_PX / 2.0;
        let t = slow_drag(&mut w, &mut state, short);
        assert!(w.offset > 0.0, "the row followed the finger");
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + short, 30.0),
            t,
        );
        assert!(state.is_empty(), "below the commit line: nothing fired");
        assert_eq!(w.motion, Motion::SpringBack);

        // The spring-back leg lands the row back at rest.
        let mut t_ms = 0u64;
        loop {
            let (_, needs_frame, _) = paint_at(&mut w, t_ms, None);
            if !needs_frame {
                break;
            }
            t_ms += 16;
            assert!(t_ms < 10_000, "the spring-back should settle inside 10s");
        }
        assert_eq!(w.offset, 0.0);
        assert_eq!(w.motion, Motion::Rest);
        assert_eq!(w.collapse, 0.0, "a spring-back never collapses the row");
    }

    // ---- Release decision: velocity ---------------------------------------

    #[test]
    fn a_fast_flick_short_of_the_threshold_commits_both_directions() {
        for (dx, expected, direction) in [
            (20.0, 1u32, DismissDirection::StartToEnd),
            (-20.0, 2u32, DismissDirection::EndToStart),
        ] {
            let mut w = build(&recording());
            layout(&mut w);
            let mut state: Vec<u32> = Vec::new();
            // 20px short of the 60px commit line, but covered in 20ms —
            // 1000 px/s, well past DISMISS_FLING_VELOCITY.
            at(
                &mut w,
                &mut state,
                PointerPhase::Down,
                Point::new(150.0, 30.0),
                0.0,
            );
            at(
                &mut w,
                &mut state,
                PointerPhase::Move,
                Point::new(150.0 + dx, 30.0),
                20.0,
            );
            at(
                &mut w,
                &mut state,
                PointerPhase::Up,
                Point::new(150.0 + dx, 30.0),
                20.0,
            );
            assert!(
                w.progress_at(w.offset) < 1.0,
                "the drag stayed short of the threshold"
            );
            assert_eq!(state, vec![expected]);
            assert_eq!(w.exit_direction, Some(direction));
        }
    }

    #[test]
    fn the_speed_multiplier_matches_the_reference_ramp() {
        assert_eq!(speed_multiplier(0.0), 1.0);
        assert_eq!(speed_multiplier(1000.0), 2.0);
        assert_eq!(speed_multiplier(-1000.0), 2.0, "sign-independent");
        assert_eq!(speed_multiplier(10_000.0), 4.0, "clamped at 4");
        assert_eq!(speed_multiplier(f64::NAN), 1.0, "never a NaN stiffness");
    }

    // ---- Axis arbitration --------------------------------------------------

    #[test]
    fn a_vertical_drag_never_resolves_into_a_horizontal_dismiss() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        at(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(150.0, 30.0),
            0.0,
        );
        // Straight down, well past the slop.
        for (step, t) in [(1.0, 100.0), (2.0, 200.0), (3.0, 300.0)] {
            at(
                &mut w,
                &mut state,
                PointerPhase::Move,
                Point::new(150.0, 30.0 + step * TOUCH_SLOP),
                t,
            );
        }
        assert_eq!(w.gesture, Gesture::Declined);
        assert_eq!(w.offset, 0.0, "a vertical drag never displaces the row");

        // A later, unmistakably horizontal move cannot revive the gesture: the
        // enclosing scroll surface already owns it.
        at(
            &mut w,
            &mut state,
            PointerPhase::Move,
            Point::new(150.0 + WIDTH, 30.0 + 3.0 * TOUCH_SLOP),
            400.0,
        );
        assert_eq!(w.gesture, Gesture::Declined);
        assert_eq!(w.offset, 0.0);

        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + WIDTH, 30.0 + 3.0 * TOUCH_SLOP),
            420.0,
        );
        assert!(state.is_empty(), "a vertical gesture dismisses nothing");
        assert_eq!(w.motion, Motion::Rest, "and starts no exit or spring-back");
    }

    /// The composed proof of the arbitration rule, through a real
    /// `ScrollView` + `Column` + `RenderRoot`: a vertical gesture over a row
    /// scrolls and dismisses nothing, and a horizontal one over the same row
    /// dismisses without scrolling.
    #[test]
    fn inside_a_scroll_view_a_vertical_drag_scrolls_and_a_horizontal_one_dismisses() {
        #[derive(Default)]
        struct S {
            dismissed: Vec<usize>,
            offset: f64,
        }

        fn logic(_s: &mut S) -> AnyView<S> {
            let rows: Vec<AnyView<S>> = (0..8)
                .map(|i| {
                    any(
                        dismissible(frust::SizedBox::<S>(Some(WIDTH), Some(ROW_HEIGHT)))
                            .on_dismissed(move |s: &mut S, _| s.dismissed.push(i)),
                    )
                })
                .collect();
            any(frust::scroll_view(frust::Column(rows))
                .on_scroll(|s: &mut S, info| s.offset = info.offset))
        }

        let ev = |phase, x: f64, y: f64| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            })
        };
        let boot = || {
            let mut root: frust_core::RenderRoot<S, AnyView<S>> = frust_core::RenderRoot::new();
            let mut state = S::default();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = TextContext::new();
            root.layout_with_text(Size::new(WIDTH, 200.0), &mut tcx as &mut dyn Any);
            (root, state)
        };

        // Vertical: the scroll surface takes it, nothing dismisses.
        let (mut root, mut state) = boot();
        root.event(&mut state, &ev(PointerPhase::Down, 150.0, 30.0));
        for step in [3.0, 6.0] {
            root.event(
                &mut state,
                &ev(PointerPhase::Move, 150.0, 30.0 - step * TOUCH_SLOP),
            );
        }
        root.event(
            &mut state,
            &ev(PointerPhase::Up, 150.0, 30.0 - 6.0 * TOUCH_SLOP),
        );
        assert!(
            state.dismissed.is_empty(),
            "a vertical scroll gesture dismissed a row"
        );
        assert!(
            state.offset > 0.0,
            "the scroll surface should have taken the gesture, offset {}",
            state.offset
        );

        // Horizontal over the same row: it dismisses, and nothing scrolls.
        let (mut root, mut state) = boot();
        root.event(&mut state, &ev(PointerPhase::Down, 150.0, 30.0));
        root.event(
            &mut state,
            &ev(PointerPhase::Move, 150.0 + 3.0 * TOUCH_SLOP, 30.0),
        );
        root.event(
            &mut state,
            &ev(PointerPhase::Move, 150.0 + COMMIT_PX + 10.0, 30.0),
        );
        root.event(
            &mut state,
            &ev(PointerPhase::Up, 150.0 + COMMIT_PX + 10.0, 30.0),
        );
        assert_eq!(state.dismissed, vec![0]);
        assert_eq!(state.offset, 0.0, "a horizontal dismiss never scrolls");
    }

    #[test]
    fn a_move_inside_the_slop_resolves_neither_axis() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        at(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(150.0, 30.0),
            0.0,
        );
        at(
            &mut w,
            &mut state,
            PointerPhase::Move,
            Point::new(150.0 + TOUCH_SLOP / 2.0, 30.0),
            50.0,
        );
        assert!(matches!(w.gesture, Gesture::Pending { .. }));
        assert_eq!(w.offset, 0.0);
    }

    #[test]
    fn a_diagonal_drag_resolves_to_whichever_axis_dominates() {
        // Mostly horizontal: dismissible takes it.
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        at(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(150.0, 30.0),
            0.0,
        );
        at(
            &mut w,
            &mut state,
            PointerPhase::Move,
            Point::new(150.0 + 3.0 * TOUCH_SLOP, 30.0 + TOUCH_SLOP),
            100.0,
        );
        assert!(matches!(w.gesture, Gesture::Dragging { .. }));

        // Mostly vertical: declined.
        let mut w = build(&recording());
        layout(&mut w);
        at(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(150.0, 30.0),
            0.0,
        );
        at(
            &mut w,
            &mut state,
            PointerPhase::Move,
            Point::new(150.0 + TOUCH_SLOP, 30.0 + 3.0 * TOUCH_SLOP),
            100.0,
        );
        assert_eq!(w.gesture, Gesture::Declined);
    }

    #[test]
    fn a_disallowed_direction_never_resolves_horizontally() {
        let mut w = build(&recording().only(DismissDirection::EndToStart));
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, COMMIT_PX * 2.0);
        assert!(
            matches!(w.gesture, Gesture::Pending { .. }),
            "a rightward drag stays unresolved on an end→start-only row"
        );
        assert_eq!(w.offset, 0.0);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + COMMIT_PX * 2.0, 30.0),
            t,
        );
        assert!(state.is_empty());
    }

    #[test]
    fn a_disabled_row_forwards_every_pass_untouched() {
        let mut w = build(&recording().enabled(false));
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, COMMIT_PX * 2.0);
        assert_eq!(w.gesture, Gesture::None);
        assert_eq!(w.offset, 0.0);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + COMMIT_PX * 2.0, 30.0),
            t,
        );
        assert!(state.is_empty());
    }

    #[test]
    fn a_secondary_press_starts_no_drag() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let event = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(150.0, 30.0),
            button: PointerButton::Secondary,
        });
        let sa: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(WIDTH, ROW_HEIGHT));
        w.event_at(&mut ctx, &event, 0.0);
        assert_eq!(w.gesture, Gesture::None);
    }

    #[test]
    fn a_cancel_mid_drag_springs_back_without_firing() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, COMMIT_PX * 2.0);
        assert!(
            w.offset > COMMIT_PX,
            "past the commit line under the finger"
        );
        at(
            &mut w,
            &mut state,
            PointerPhase::Cancel,
            Point::new(150.0 + COMMIT_PX * 2.0, 30.0),
            t,
        );
        assert!(state.is_empty(), "a Cancel never fires the callback");
        assert_eq!(w.motion, Motion::SpringBack);
    }

    // ---- The exit: slide, then collapse -----------------------------------

    #[test]
    fn the_exit_slides_fully_out_before_the_height_starts_collapsing() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, COMMIT_PX + 10.0);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + COMMIT_PX + 10.0, 30.0),
            t,
        );
        assert_eq!(state, vec![1]);
        let target = w.leg_to;
        assert!(
            (target - (WIDTH + DISMISS_FLY_OVERSHOOT)).abs() < 1e-9,
            "the slide targets the row's width plus the overshoot"
        );

        let mut t_ms = 0u64;
        let mut collapse_started_at: Option<f64> = None;
        let mut frames = 0u32;
        loop {
            let before = w.collapse;
            let (_, needs_frame, needs_layout) = paint_at(&mut w, t_ms, None);
            if w.collapse > 0.0 && before == 0.0 {
                // Record how far the slide had travelled the first time the
                // height moved at all.
                collapse_started_at = Some((w.offset - 0.0) / target);
                assert!(needs_layout, "a collapsing row asks for a relayout");
            }
            if !needs_frame && !needs_layout {
                break;
            }
            frames += 1;
            t_ms += 16;
            assert!(frames < 1000, "the exit should settle inside 1000 frames");
        }
        let handoff = collapse_started_at.expect("the height collapsed at some point");
        assert!(
            handoff >= DISMISS_COLLAPSE_HANDOFF - 0.05,
            "the collapse waits for the slide to be ~90% done, started at {handoff}"
        );
        assert_eq!(w.collapse, 1.0, "the row ends fully collapsed");
        assert_eq!(w.motion, Motion::Rest);
        // A fully-collapsed row reports no height at all.
        assert_eq!(layout(&mut w).height, 0.0);
    }

    #[test]
    fn a_row_mid_exit_ignores_a_fresh_press() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let t = slow_drag(&mut w, &mut state, COMMIT_PX + 10.0);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + COMMIT_PX + 10.0, 30.0),
            t,
        );
        assert_eq!(w.motion, Motion::Exit);
        at(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(150.0, 30.0),
            t + 10.0,
        );
        assert_eq!(w.gesture, Gesture::None, "the exit owns the row");
        assert_eq!(w.motion, Motion::Exit);
    }

    #[test]
    fn without_on_dismissed_a_committed_release_still_only_springs_back() {
        let mut w = build(&view());
        layout(&mut w);
        let mut state: u32 = 0;
        let t = slow_drag(&mut w, &mut state, COMMIT_PX * 2.0);
        at(
            &mut w,
            &mut state,
            PointerPhase::Up,
            Point::new(150.0 + COMMIT_PX * 2.0, 30.0),
            t,
        );
        assert_eq!(w.motion, Motion::SpringBack);
    }

    // ---- Paint: the revealed band -----------------------------------------

    #[test]
    fn no_band_is_painted_at_rest() {
        let mut w =
            build(&recording().background(dismiss_background().color(Color::from_rgb8(1, 2, 3))));
        layout(&mut w);
        let (rec, _, _) = paint_at(&mut w, 0, None);
        assert!(rec.rrects.is_empty());
        assert_eq!(rec.transforms, vec![Affine::translate(Vec2::new(0.0, 0.0))]);
    }

    #[test]
    fn the_band_reveals_from_the_leading_edge_on_a_rightward_drag() {
        let mut w =
            build(&recording().background(dismiss_background().color(Color::from_rgb8(1, 2, 3))));
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        slow_drag(&mut w, &mut state, COMMIT_PX);
        let (rec, _, _) = paint_at(&mut w, 0, None);
        let (band_origin, band_size, radius, _) = rec.rrects[0];
        assert_eq!(band_origin.x, 0.0, "anchored to the leading edge");
        assert!((band_size.width - (COMMIT_PX - DISMISS_ACTION_GAP)).abs() < 1e-9);
        assert_eq!(band_size.height, ROW_HEIGHT);
        assert_eq!(radius, DISMISS_BACKGROUND_RADIUS);
        // The row itself is translated by the drag.
        assert_eq!(
            rec.transforms,
            vec![Affine::translate(Vec2::new(COMMIT_PX, 0.0))]
        );
    }

    #[test]
    fn the_secondary_band_reveals_from_the_trailing_edge_on_a_leftward_drag() {
        let mut w = build(
            &recording()
                .background(dismiss_background().color(Color::from_rgb8(1, 2, 3)))
                .secondary_background(dismiss_background().color(Color::from_rgb8(9, 9, 9))),
        );
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        slow_drag(&mut w, &mut state, -COMMIT_PX);
        let (rec, _, _) = paint_at(&mut w, 0, None);
        let (band_origin, band_size, _, color) = rec.rrects[0];
        let expected_width = COMMIT_PX - DISMISS_ACTION_GAP;
        assert!((band_origin.x - (WIDTH - expected_width)).abs() < 1e-9);
        assert!((band_size.width - expected_width).abs() < 1e-9);
        let secondary = Color::from_rgb8(9, 9, 9).components;
        assert_eq!(
            [
                color.components[0],
                color.components[1],
                color.components[2]
            ],
            [secondary[0], secondary[1], secondary[2]],
            "the leftward band uses secondary_background, not background"
        );
    }

    #[test]
    fn a_leftward_drag_falls_back_to_the_primary_band_when_no_secondary_is_set() {
        let mut w =
            build(&recording().background(dismiss_background().color(Color::from_rgb8(1, 2, 3))));
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        slow_drag(&mut w, &mut state, -COMMIT_PX);
        let (rec, _, _) = paint_at(&mut w, 0, None);
        assert_eq!(rec.rrects.len(), 1, "the primary band stands in");
    }

    #[test]
    fn the_band_icon_only_fades_in_past_the_ramp_start() {
        // A wide commit distance (half the row) so the sub-`ICON_FADE_START`
        // window is reachable at all: at the default 20% threshold the commit
        // line is 60px and `TOUCH_SLOP` already puts the first resolved frame
        // past 30% of it.
        let wide_commit = WIDTH * 0.5;
        let with_icon = || {
            recording().threshold(0.5).background(
                dismiss_background()
                    .color(Color::from_rgb8(1, 2, 3))
                    .icon(crate::icons::DELETE),
            )
        };
        // Below the fade start: the band shows, the icon does not.
        let mut w = build(&with_icon());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        let short = wide_commit * (ICON_FADE_START / 2.0);
        slow_drag(&mut w, &mut state, short);
        assert!(w.progress_at(w.offset) < ICON_FADE_START);
        assert!(short - DISMISS_ACTION_GAP > 0.0, "the band has real width");
        let (rec, _, _) = paint_at(&mut w, 0, None);
        assert_eq!(rec.rrects.len(), 1);
        assert!(rec.paths.is_empty(), "the icon is still fully transparent");

        // At the commit line: band plus icon.
        let mut w = build(&with_icon());
        layout(&mut w);
        slow_drag(&mut w, &mut state, wide_commit);
        assert_eq!(w.progress_at(w.offset), 1.0);
        let (rec, _, _) = paint_at(&mut w, 0, None);
        assert_eq!(rec.paths.len(), 1, "the icon is painted at full progress");
    }

    #[test]
    fn a_row_without_a_background_paints_no_band() {
        let mut w = build(&recording());
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        slow_drag(&mut w, &mut state, COMMIT_PX);
        let (rec, _, _) = paint_at(&mut w, 0, None);
        assert!(rec.rrects.is_empty());
    }

    #[test]
    fn the_band_resolves_the_error_container_roles_from_a_theme() {
        let theme = crate::baseline();
        let mut w = build(&recording().background(dismiss_background()));
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        slow_drag(&mut w, &mut state, COMMIT_PX);
        let (rec, _, _) = paint_at(&mut w, 0, Some(&theme));
        let expected = theme.scheme().error_container.components;
        let painted = rec.rrects[0].3.components;
        assert_eq!(
            [painted[0], painted[1], painted[2]],
            [expected[0], expected[1], expected[2]]
        );
    }

    // ---- Layout ------------------------------------------------------------

    #[test]
    fn an_untouched_row_reports_its_childs_height() {
        let mut w = build(&view());
        let size = layout(&mut w);
        assert_eq!(size, Size::new(WIDTH, ROW_HEIGHT));
    }

    #[test]
    fn a_half_collapsed_row_reports_half_its_height() {
        let mut w = build(&view());
        layout(&mut w);
        w.collapse = 0.5;
        assert_eq!(layout(&mut w).height, ROW_HEIGHT / 2.0);
        assert_eq!(
            w.child_height, ROW_HEIGHT,
            "the child stays laid out at its own height"
        );
    }

    #[test]
    fn the_child_is_clipped_to_the_reported_height() {
        let mut w = build(&view());
        layout(&mut w);
        w.collapse = 0.5;
        let (rec, _, _) = paint_at(&mut w, 0, None);
        assert_eq!(
            rec.clips,
            vec![(Point::ZERO, Size::new(WIDTH, ROW_HEIGHT / 2.0))]
        );
    }

    #[test]
    fn semantics_forwards_the_child_without_a_node_of_its_own() {
        fn logic(_s: &mut ()) -> DismissibleView<()> {
            dismissible(frust::text("row")).on_dismissed(|_: &mut (), _| {})
        }
        let mut root: frust_core::RenderRoot<(), DismissibleView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(WIDTH, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            !update.nodes.is_empty(),
            "the wrapped row's own nodes still reach the tree"
        );
    }

    #[test]
    fn leaving_the_row_disabled_mid_drag_clears_the_gesture() {
        let enabled = recording();
        let mut w = build(&enabled);
        layout(&mut w);
        let mut state: Vec<u32> = Vec::new();
        slow_drag(&mut w, &mut state, COMMIT_PX);
        assert!(matches!(w.gesture, Gesture::Dragging { .. }));
        let disabled = recording().enabled(false);
        let mut counter = 0u64;
        <DismissibleView<Vec<u32>> as View<Vec<u32>>>::rebuild(
            &disabled,
            &enabled,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(w.gesture, Gesture::None);
        assert_eq!(w.offset, 0.0);
        assert_eq!(w.motion, Motion::Rest);
    }
}
