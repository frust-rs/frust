//! The Material 3 Expressive bottom `NavigationBar`: two container heights,
//! three label behaviors, three selection-indicator styles, per-item badges,
//! and the **liquid selection indicator** — a two-spring pill that stretches
//! into a bridge between the outgoing and incoming destination and then
//! settles onto the new one.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/navigation_bar/` (`m3e_navigation_bar.dart`,
//! `enums/m3e_nav_bar_enums.dart`, `models/m3e_nav_metrics.dart`,
//! `models/m3e_navigation_bar_destination.dart`,
//! `styles/m3e_navigation_bar_theme.dart`,
//! `components/m3e_nav_bar_destination_button.dart`,
//! `components/m3e_nav_badge_view.dart`) plus the two motion pieces the bar
//! shares with the rail (`lib/components/navigation_rail/components/
//! m3e_nav_selection_indicator.dart` and `m3e_nav_icon_scale.dart`), retrieved
//! 2026-08-20. Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Controlled selection
//!
//! `NavigationBarView`/`NavigationBarWidget` follow the widget-authoring
//! recipe: a **controlled** component — [`NavigationBarView`] reports the
//! *requested* selection through `on_select(index)` and never self-mutates; the
//! app feeds the confirmed `selected` index back in on the next rebuild,
//! exactly like [`frust::Checkbox`]/[`frust::Slider`].
//!
//! Each item is a small, self-contained retained widget
//! (`NavItemWidget`, **not** built through `View`/[`frust::authoring::AnyView`]
//! — there is only ever one concrete item type, so no type erasure is
//! needed) wrapped in its own [`frust::authoring::ChildPod`] purely so the bar
//! can route pointer/semantics through the same
//! [`frust::authoring::route_event`]/[`frust::authoring::ChildPod::semantics_child`]
//! helpers every multi-child container uses. Each item lays out an optional
//! caller-supplied `icon` (an opaque `AnyView` — tint is the caller's own
//! responsibility, mirroring [`super::appbar`]'s leading/actions slots) inside
//! a fixed [`INDICATOR_W`]×[`INDICATOR_H`] indicator box, optionally above a
//! label (a real child [`frust::text`]), vertically centred as a group inside
//! the bar's height; it fires `on_select` on release-inside, fire-on-up-inside
//! like [`frust::Button`].
//!
//! # The liquid selection indicator
//!
//! The pill is owned by the **bar**, not by the item — the crate-private
//! `LiquidIndicator` block below carries the two-spring stretch/settle
//! mechanism, its transcribed spring constants ([`LEAD_SPRING`]/
//! [`TRAIL_SPRING`]), and the two-path (travel vs. jump) sync rule; that block
//! is the tier's reference implementation, cited by the rail and drawer rather
//! than re-derived. Items paint only their icon and label; every indicator style
//! ([`NavBarIndicatorStyle`]) is painted by the bar, under the items.
//!
//! # Porting decisions
//!
//! * **One pill, not an overlay plus a resting copy.** The reference stacks a
//!   shared morph overlay over the destination row and hands the *resting*
//!   fill back to each destination (`showRestingPill`/`onTravelingChanged`),
//!   because its overlay is measured asynchronously through `GlobalKey`s a
//!   frame after layout. A retained widget has no such gap: the bar reads the
//!   selected item's own indicator box in the same layout pass and paints one
//!   pill that *is* the resting box while settled, so the handoff, the
//!   traveling flag, and the deferred `setState` machinery around it have no
//!   analogue here.
//! * **No ink splash**, matching the reference (`M3ENavBarDestinationButton`'s
//!   own note): selection feedback is the pill, so an item runs no
//!   [`super::state_layer`]/[`crate::interaction`] overlay and repaints on
//!   selection, never on press.
//! * **Default size is [`NavBarSize::Medium`]** (80dp), matching the reference
//!   default. [`NavBarSize::Small`] (64dp) — androidx `NavigationBarTokens`'
//!   current Expressive container height and the height this widget has
//!   shipped — is still reachable via `.size(NavBarSize::Small)`.
//! * **A selected label keeps the `on_surface` themed role.** The reference
//!   paints selected content `onSecondaryContainer`; [`ThemeTextColor`] has no
//!   such role, and a themed text color resolves from a *role* after `build`
//!   (there is no `Theme` to read at build time), so the closest shipped role
//!   stays. Unselected labels are `on_surface_variant` as upstream.
//! * **Unported upstream props**: `density`, `shapeFamily`, `elevation`,
//!   `padding`, and `selectedIcon` — container/decoration knobs with no axis
//!   in this port's scope. `safeArea` **is** ported: the bar consumes the
//!   bottom window inset itself by default, in its own `layout`
//!   (`docs/CODE_STANDARDS.md`'s self-sizing-chrome rule), and paints its
//!   `surface_container` fill through the consumed band; `.safe_area(false)`
//!   opts out for a bar that isn't docked to the window's bottom edge. Do not
//!   additionally wrap the bar in `frust::safe_area(..)` for the bottom edge:
//!   a `SafeArea` removes what it consumes from its subtree, so this never
//!   double-insets, but the safe area's own padding sits outside the bar's
//!   box and is left unpainted instead of matching the bar's container color.
//!
//! # Semantics
//!
//! The bar is one [`Role::TabList`] container node; each item contributes a
//! [`Role::Tab`] node labelled with its text and carrying
//! [`frust::authoring::Node::set_selected`] — the closest accesskit vocabulary to
//! "one of a set of mutually-exclusive destinations", chosen over
//! `Role::RadioGroup`/`Role::Toggled` since a bottom nav bar reads to a
//! screen reader exactly like a tab strip. The label rides the node even when
//! [`NavBarLabelBehavior::AlwaysHide`] paints no text.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::Role;
use frust::authoring::text::{FontWeight, LineHeight};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, SemanticsCtx, View,
    Widget,
};
use frust::{AnimationController, FrameTime, Spring, SpringDesc, Theme, text};
use kurbo::{Affine, Point, Rect, Size, Vec2};
use peniko::Color;

use frust::authoring::{ThemeTextColor, ThemeTextType};

use super::press::presses;
use crate::tokens::MaterialSpring;

/// Compact container height, in logical px
/// (`M3ENavigationBarTheme.heightSmall`; equal to androidx
/// `NavigationBarTokens`' current Expressive container height).
pub const HEIGHT_SMALL: f64 = 64.0;
/// Standard container height, in logical px
/// (`M3ENavigationBarTheme.heightMedium`).
pub const HEIGHT_MEDIUM: f64 = 80.0;
/// Selection indicator box width, in logical px
/// (`_M3ENavigationBarState._indicatorWidth`). The androidx "active indicator"
/// token is 56dp; the reference's own 64dp is what this port carries, since
/// every other metric here comes from the same table.
pub const INDICATOR_W: f64 = 64.0;
/// Selection indicator box height, in logical px
/// (`_M3ENavigationBarState._indicatorHeight`).
pub const INDICATOR_H: f64 = 32.0;
/// Underline thickness for [`NavBarIndicatorStyle::Underline`], in logical px
/// (`M3ENavigationBarTheme.indicatorThickness`).
pub const UNDERLINE_THICKNESS: f64 = 3.0;
/// Gap between the indicator box and the label below it, in logical px
/// (`m3e_nav_bar_destination_button.dart`'s `SizedBox(height: 4)`).
const LABEL_GAP: f64 = 4.0;

/// The label's M3 `labelMediumEmphasized` type-scale token (see
/// [`super::appbar`]'s `TITLE_SIZE` doc comment for why this is a hardcoded
/// constant rather than a live `Theme::type_scale` read — `Text` defers *color*
/// and an opt-in family past `View::build`, never size/weight). Matches
/// `frust-theme::typography`'s `LABEL_MEDIUM_EMPHASIZED` token: same
/// size/line-height/letter-spacing as the baseline `LABEL_MEDIUM`, weight
/// stepped up from Medium to Bold.
const LABEL_SIZE: f32 = 12.0;
const LABEL_LINE_HEIGHT: f32 = 16.0;
const LABEL_LETTER_SPACING: f32 = 0.5;
const LABEL_WEIGHT: FontWeight = FontWeight::BOLD;

/// Unthemed fallback container fill (a theme resolves this from
/// `colors.surface_container`).
const CONTAINER: Color = Color::from_rgb8(0xF3, 0xED, 0xF7);
/// Unthemed fallback indicator fill (a theme resolves this from
/// `colors.secondary_container`).
const INDICATOR: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);

// ---------------------------------------------------------------------------
// Liquid selection indicator (the port core)
// ---------------------------------------------------------------------------
//
// `m3e_nav_selection_indicator.dart`'s `M3ENavSelectionIndicator`, transcribed.
// The rail and drawer ports reuse this block rather than re-deriving it.

/// Leading-edge spring: the reference's
/// `MaterialSpringMotion.expressiveSpatialDefault().copyWith(damping: 0.45)`
/// (`_M3ENavSelectionIndicatorState._leadMotion`). Stiffness comes from the
/// shared [`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`] token (380); only the
/// damping ratio is overridden, and motor's `damping` is a damping *ratio* —
/// the same ζ [`SpringDesc::damping_ratio`] takes, so it carries over with no
/// conversion. Lower ζ than the trail: this edge runs ahead.
pub const LEAD_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: 0.45,
};
/// Trailing-edge spring: the same preset at `damping: 0.55`
/// (`_M3ENavSelectionIndicatorState._trailMotion`). Higher ζ than the lead, so
/// this edge lags — the gap between the two *is* the stretch.
pub const TRAIL_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: 0.55,
};
/// Rest threshold for a travelling edge, in logical px (and px/s for its
/// velocity) — both are checked, per `Spring::is_at_rest`'s own contract. The
/// reference leans on motor's default simulation tolerance (1e-3) for the same
/// job; this unit is a *logical pixel*, so a hundredth of one is already an
/// order of magnitude below anything a display can resolve, and stopping there
/// saves the long analytic tail of frames that changes nothing on screen.
const PILL_REST_EPSILON: f64 = 0.01;
/// Movement below this (logical px) doesn't count as a geometry change — the
/// reference's own 0.5px `_isAtGeometry` test
/// (`_M3ENavSelectionIndicatorState`). Gates two things: a travel retarget
/// within this of the live lead position is skipped (a re-selection of the
/// already-selected destination, or a rounding-scale slot shift), and
/// [`geometry_unchanged`] uses it to tell a genuine resize/add/remove apart
/// from a same-geometry relayout in [`LiquidIndicator::sync`]'s jump branch.
const GEOMETRY_EPSILON: f64 = 0.5;

/// Whether `a` and `b` are the same resting rect within [`GEOMETRY_EPSILON`]
/// on every edge — the "did this relayout actually move anything" test
/// gating [`LiquidIndicator::sync`]'s non-travel jump.
fn geometry_unchanged(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < GEOMETRY_EPSILON
        && (a.x1 - b.x1).abs() < GEOMETRY_EPSILON
        && (a.y0 - b.y0).abs() < GEOMETRY_EPSILON
        && (a.y1 - b.y1).abs() < GEOMETRY_EPSILON
}

/// One spring-driven edge of the liquid pill, in container-local **main-axis**
/// px — a bare scalar, so it carries over to a vertical consumer unchanged.
///
/// A [`Spring`] solves *displacement from equilibrium*, so the live position is
/// `target + spring.position(elapsed)`. Retargeting mid-flight re-solves from
/// the current position **and velocity**, which is what motor's
/// `SingleMotionController.animateTo` does and what keeps a fast double-tap
/// between destinations from snapping.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LiquidEdge {
    desc: SpringDesc,
    /// Equilibrium this edge is settling toward.
    target: f64,
    /// The in-flight solution, or `None` once settled.
    flight: Option<Spring>,
    /// Seconds since [`Self::flight`] was solved.
    elapsed: f64,
    position: f64,
    velocity: f64,
}

impl LiquidEdge {
    pub(crate) const fn new(desc: SpringDesc) -> Self {
        LiquidEdge {
            desc,
            target: 0.0,
            flight: None,
            elapsed: 0.0,
            position: 0.0,
            velocity: 0.0,
        }
    }

    /// Jump to `to` with no motion (`_jumpToGeometry`).
    pub(crate) fn snap(&mut self, to: f64) {
        self.target = to;
        self.position = to;
        self.velocity = 0.0;
        self.flight = None;
        self.elapsed = 0.0;
    }

    /// Start (or re-aim) a travel toward `to`, continuing from the current
    /// position and velocity (`_animateToGeometry`).
    pub(crate) fn retarget(&mut self, to: f64) {
        self.flight = Some(Spring::new(self.desc, self.position - to, self.velocity));
        self.target = to;
        self.elapsed = 0.0;
    }

    /// Advance `dt` seconds, returning whether the edge is still travelling.
    pub(crate) fn advance(&mut self, dt: f64) -> bool {
        let Some(spring) = self.flight else {
            return false;
        };
        self.elapsed += dt;
        if spring.is_at_rest(self.elapsed, PILL_REST_EPSILON) {
            self.settle();
            return false;
        }
        self.position = self.target + spring.position(self.elapsed);
        self.velocity = spring.velocity(self.elapsed);
        true
    }

    /// Land on the target immediately, dropping any in-flight solution.
    pub(crate) fn settle(&mut self) {
        self.position = self.target;
        self.velocity = 0.0;
        self.flight = None;
        self.elapsed = 0.0;
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.flight.is_some()
    }
}

/// The bar's liquid selection indicator: two independently-sprung edge centres
/// whose span is the painted pill.
///
/// # The two-phase motion
///
/// Both edges chase the *same* target — the newly selected item's indicator-box
/// centre — but through different springs ([`LEAD_SPRING`] ζ 0.45,
/// [`TRAIL_SPRING`] ζ 0.55). So:
///
/// 1. **Stretch.** The lead edge accelerates away first and the trail lags, so
///    `|lead − trail|` grows: the pill's main extent is
///    `|lead − trail| + rest_width` (`_buildPill`), a bridge spanning from the
///    outgoing destination toward the incoming one.
/// 2. **Settle.** The trail catches the lead up; the gap collapses, the extent
///    returns to `rest_width`, and the pill lands centred on the new
///    destination. Both edges are then at rest and no further frame is asked
///    for.
///
/// The painted rect is always `[min(lead, trail) − rest_width/2, max(lead,
/// trail) + rest_width/2]` on the main axis and the selected item's own box on
/// the cross axis, with a corner radius of `min(cross, rest_width)/2` — the
/// reference's `_buildPill` verbatim, which is why a settled pill *is* exactly
/// the resting box.
///
/// # Two sync paths
///
/// [`Self::sync`] takes the same fork the reference's `_sync(forceJump:)` does:
/// a **selection change** animates (`didUpdateWidget`'s `selectedIndex` arm),
/// while anything else that *moves the geometry* — a width change, an item
/// added/removed, first layout, reduced motion — **jumps**
/// (`_trackLayoutChange`/`_scheduleMeasure(forceJump: true)`). Measuring is
/// synchronous here, so the reference's retry/suspicious-jump guards
/// (`_measureAttempts`, `_isSuspiciousJump`) have no analogue: there is no
/// frame in which the geometry is not yet knowable.
///
/// A non-travel call whose `rest` reproduces the last-synced one (within
/// [`GEOMETRY_EPSILON`]) is a **no-op**, matching the reference's own
/// `_isAtGeometry` guard (`m3e_nav_selection_indicator.dart`). `layout` runs
/// on every full-tree layout pass, not just the one where the selection
/// actually moved — an animating sibling's `request_layout`, or any other
/// app-side relayout, re-enters [`Self::sync`] with `travel == false` while a
/// previously-started travel is still mid-flight. Since the resting rect
/// hasn't actually changed, this must leave the in-flight travel (or a
/// settled pill) exactly as it was — only a call that reproduces a *genuinely
/// different* resting rect jumps.
///
/// # Reuse across the nav family
///
/// This type and [`LiquidEdge`] are crate-visible on purpose: the reference
/// shares one `M3ENavSelectionIndicator` between the bar, the rail, and the
/// drawer, and so does this port. Everything above is main-axis scalar work
/// and carries over unchanged; the **only** horizontal-specific piece is the
/// main↔`x` / cross↔`y` mapping inside [`Self::sync`] and [`Self::pill_rect`],
/// which a vertical consumer swaps (the same primary-axis split
/// [`crate::slider`]'s core documents).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LiquidIndicator {
    lead: LiquidEdge,
    trail: LiquidEdge,
    /// Clock for both edges; `None` re-seeds the delta on the next advance.
    last_time: Option<FrameTime>,
    /// The selected item's resting indicator box, in bar-local px.
    rest: Rect,
    /// Whether [`Self::rest`] has ever been resolved (pre-first-layout, the
    /// indicator paints nothing).
    ready: bool,
}

impl LiquidIndicator {
    pub(crate) const fn new() -> Self {
        LiquidIndicator {
            lead: LiquidEdge::new(LEAD_SPRING),
            trail: LiquidEdge::new(TRAIL_SPRING),
            last_time: None,
            rest: Rect::ZERO,
            ready: false,
        }
    }

    /// Adopt the selected item's resting box. `travel` runs the two-phase
    /// motion; otherwise the pill jumps to a *genuine* geometry change and is
    /// a no-op against a reproduction of the last-synced one (see the type
    /// docs' two sync paths).
    pub(crate) fn sync(&mut self, rest: Rect, travel: bool) {
        let center = rest.center().x;
        if !self.ready {
            self.rest = rest;
            self.lead.snap(center);
            self.trail.snap(center);
            self.last_time = None;
            self.ready = true;
            return;
        }
        if travel {
            self.rest = rest;
            if (self.lead.target - center).abs() < GEOMETRY_EPSILON && !self.is_animating() {
                return;
            }
            self.lead.retarget(center);
            self.trail.retarget(center);
            self.last_time = None;
            return;
        }
        // Not a selection-driven travel. A relayout that reproduces the
        // resting rect already synced (an unrelated widget's animating
        // frame, any other app-side LAYOUT flag) touches nothing — snapping
        // here would cancel an in-flight travel mid-stretch. Only a rect
        // that actually differs — a resize, an item add/remove — jumps.
        if geometry_unchanged(self.rest, rest) {
            return;
        }
        self.rest = rest;
        self.lead.snap(center);
        self.trail.snap(center);
        self.last_time = None;
    }

    /// Advance both edges to frame time `now`, returning whether the pill is
    /// still travelling (i.e. whether the caller must ask for another frame).
    pub(crate) fn advance(&mut self, now: FrameTime) -> bool {
        let dt = match self.last_time {
            Some(last) => now.saturating_sub(last).as_secs_f64(),
            None => 0.0,
        };
        self.last_time = Some(now);
        let lead = self.lead.advance(dt);
        let trail = self.trail.advance(dt);
        lead || trail
    }

    /// Land the pill on its target immediately (reduced motion).
    pub(crate) fn settle(&mut self) {
        self.lead.settle();
        self.trail.settle();
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.lead.is_animating() || self.trail.is_animating()
    }

    /// The painted pill in bar-local px, or `None` before first layout.
    pub(crate) fn pill_rect(&self) -> Option<Rect> {
        if !self.ready {
            return None;
        }
        let base = self.rest.width();
        let min = self.lead.position.min(self.trail.position);
        let max = self.lead.position.max(self.trail.position);
        let x0 = min - base / 2.0;
        Some(Rect::new(
            x0,
            self.rest.y0,
            x0 + (max - min) + base,
            self.rest.y1,
        ))
    }

    /// Corner radius of the pill: `math.min(_crossSize, _baseMain) / 2`, held
    /// at the *resting* size so a stretched bridge keeps stadium ends.
    pub(crate) fn radius(&self) -> f64 {
        self.rest.height().min(self.rest.width()) / 2.0
    }
}

// ---------------------------------------------------------------------------
// Icon selection pop (`m3e_nav_icon_scale.dart`)
// ---------------------------------------------------------------------------

/// The newly-selected icon's pop spring: the reference's
/// `expressiveSpatialDefault().copyWith(damping: 0.5)`
/// (`_M3ENavIconScaleState._motion`).
const ICON_POP_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: 0.5,
};
/// Scale the pop starts from (`_M3ENavIconScaleState.didUpdateWidget`'s
/// `..value = 0.98`, then `animateTo(1)`): the spring's own overshoot past
/// `1.0` is the pop — it never dips below the resting scale first.
const ICON_POP_FROM: f64 = 0.98;
/// Launch velocity for a pop. Sign-only, per the convention
/// [`crate::expandable_list`]'s `REVEAL_LAUNCH_VELOCITY` documents: the
/// spring's shape comes from its displacement, and the leg always runs
/// `0 → 1` in progress space.
const ICON_POP_LAUNCH_VELOCITY: f64 = 1e-3;
/// Scale deviation below which the pop transform is skipped entirely.
const ICON_POP_EPSILON: f64 = 1e-4;

/// A pop controller parked at its settled value (scale `1.0`) — the state
/// every item builds in and returns to.
fn settled_pop() -> AnimationController {
    let mut pop = AnimationController::new(Duration::ZERO);
    pop.animate_to(1.0);
    pop.advance(FrameTime::ZERO); // zero-duration: settles instantly.
    pop
}

/// A fresh pop controller launched from [`ICON_POP_FROM`].
fn launched_pop() -> AnimationController {
    let mut pop = AnimationController::new(Duration::ZERO);
    pop.fling(ICON_POP_LAUNCH_VELOCITY, ICON_POP_SPRING);
    pop
}

// ---------------------------------------------------------------------------
// Public props
// ---------------------------------------------------------------------------

/// The bar's container height variant (`M3ENavBarSize`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavBarSize {
    /// [`HEIGHT_SMALL`] (64dp) — androidx `NavigationBarTokens`' Expressive container height,
    /// reachable via `.size(NavBarSize::Small)`; see the [module docs](self)' porting decisions.
    Small,
    /// [`HEIGHT_MEDIUM`] (80dp), the reference's own default — this port's default.
    #[default]
    Medium,
}

impl NavBarSize {
    /// This variant's container height, in logical px.
    pub fn height(self) -> f64 {
        match self {
            NavBarSize::Small => HEIGHT_SMALL,
            NavBarSize::Medium => HEIGHT_MEDIUM,
        }
    }
}

/// When destination labels are painted (`M3ENavBarLabelBehavior`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavBarLabelBehavior {
    /// Every destination shows its label (`alwaysShow`).
    #[default]
    AlwaysShow,
    /// Only the selected destination shows its label (`onlySelected`).
    OnlySelected,
    /// No destination shows a label (`alwaysHide`); the text still rides the
    /// item's semantics node.
    AlwaysHide,
}

/// The selection indicator's visual style (`M3ENavBarIndicatorStyle`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavBarIndicatorStyle {
    /// The liquid stadium pill behind the icon (`pill`) — the only style that
    /// animates; see the [module docs](self)' liquid-indicator section.
    #[default]
    Pill,
    /// A [`UNDERLINE_THICKNESS`]-thick rule along the bottom edge of the
    /// selected destination's indicator box (`underline`).
    Underline,
    /// No indicator at all (`none`).
    None,
}

/// A destination's badge (`M3ENavigationBarDestination.badgeDot`/`badgeCount`),
/// composed onto the item's icon by [`navigation_bar`] through
/// [`crate::badge`] — the same delegation `M3ENavBadge` makes to `M3EBadge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavBadge {
    /// An unlabeled dot (`badgeDot: true`).
    Dot,
    /// A count, formatted against [`crate::BadgeView::max_count`]'s default
    /// (`badgeCount`).
    Count(u32),
}

/// The resolved `(container, indicator)` fills. Themed:
/// `surface_container`/`secondary_container`
/// (`M3ENavigationBarTheme.containerColor`/`indicatorColor`). Unthemed: the
/// [`CONTAINER`]/[`INDICATOR`] constants exactly.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.surface_container, scheme.secondary_container)
        }
        None => (CONTAINER, INDICATOR),
    }
}

/// Build a label's type-erased child view at `labelMediumEmphasized`, tagged
/// with the themed color `role` selection determines, its font family
/// following the live theme's `labelMediumEmphasized` role.
fn label_view<State: 'static>(label: String, role: ThemeTextColor) -> AnyView<State> {
    frust::authoring::any::<State, _>(
        text(label)
            .size(LABEL_SIZE)
            .weight(LABEL_WEIGHT)
            .letter_spacing(LABEL_LETTER_SPACING)
            .line_height(LineHeight::Absolute(LABEL_LINE_HEIGHT))
            .themed_role(role)
            .themed_family(ThemeTextType::LabelMediumEmphasized),
    )
}

/// The themed color role a destination's label takes
/// (`M3ENavigationBarTheme.selectedColor`/`unselectedColor`, see the [module
/// docs](self)' note on the selected role).
fn label_role(selected: bool) -> ThemeTextColor {
    if selected {
        ThemeTextColor::OnSurface
    } else {
        ThemeTextColor::OnSurfaceVariant
    }
}

/// Whether a destination paints its label
/// (`M3ENavBarDestinationButton._showLabel`).
fn label_shown(behavior: NavBarLabelBehavior, selected: bool) -> bool {
    match behavior {
        NavBarLabelBehavior::AlwaysShow => true,
        NavBarLabelBehavior::OnlySelected => selected,
        NavBarLabelBehavior::AlwaysHide => false,
    }
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (mirrors [`frust::button`]'s helper of the same shape).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// One destination's declarative content: an optional leading icon (an
/// opaque [`AnyView`] — tint is the caller's own responsibility), a text
/// label, and an optional badge.
pub struct NavItem<State: 'static> {
    icon: Option<AnyView<State>>,
    label: String,
    badge: Option<NavBadge>,
}

/// Create a nav item labelled `label`, with no icon (attach one with
/// [`NavItem::icon`]).
pub fn nav_item<State: 'static>(label: impl Into<String>) -> NavItem<State> {
    NavItem {
        icon: None,
        label: label.into(),
        badge: None,
    }
}

impl<State: 'static> NavItem<State> {
    /// Attach a leading icon, erased as an [`AnyView`]. Tint is the supplied
    /// view's own responsibility — see the [module docs](self).
    pub fn icon(mut self, icon: impl View<State>) -> Self {
        self.icon = Some(AnyView::new(icon));
        self
    }

    /// Show an unlabeled badge dot over the icon
    /// (`M3ENavigationBarDestination.badgeDot`). Wins over
    /// [`NavItem::badge_count`], whichever is set last; an item with no icon
    /// has nothing to badge and ignores it.
    pub fn badge_dot(mut self) -> Self {
        self.badge = Some(NavBadge::Dot);
        self
    }

    /// Show a numeric badge over the icon
    /// (`M3ENavigationBarDestination.badgeCount`), formatted by
    /// [`mod@crate::badge`]'s own overflow rule.
    pub fn badge_count(mut self, count: u32) -> Self {
        self.badge = Some(NavBadge::Count(count));
        self
    }

    /// Fold this item's badge onto its icon view
    /// (`M3ENavigationBarDestination.buildIcon`).
    ///
    /// Composition happens exactly once, when [`navigation_bar`] takes
    /// ownership of the item list — not inside a builder method, which could
    /// wrap a second time on a later `badge_*` call.
    fn compose_badge(mut self) -> Self {
        let Some(kind) = self.badge else {
            return self;
        };
        let Some(icon) = self.icon.take() else {
            return self;
        };
        let badged = crate::badge::badge::<State, _>(icon);
        let badged = match kind {
            NavBadge::Dot => badged.dot(),
            NavBadge::Count(count) => badged.count(count),
        };
        self.icon = Some(frust::authoring::any::<State, _>(badged));
        self
    }
}

/// A view-held, typed selection callback (erased per-item on build/rebuild).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative M3 Expressive bottom NavigationBar. See the [module docs](self).
pub struct NavigationBarView<State: 'static> {
    items: Vec<NavItem<State>>,
    selected: usize,
    on_select: OnSelect<State>,
    size: NavBarSize,
    label_behavior: NavBarLabelBehavior,
    indicator_style: NavBarIndicatorStyle,
    safe_area: bool,
}

/// Create a navigation bar over `items`, with `selected` the current
/// (app-confirmed) index. Fires `on_select(state, index)` on a release inside
/// an item — a **controlled** component: `selected` is never mutated by this
/// widget itself; the app must feed the confirmed index back in via the next
/// rebuild.
pub fn navigation_bar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<NavItem<State>>,
    selected: usize,
    on_select: F,
) -> NavigationBarView<State> {
    NavigationBarView {
        items: items
            .into_iter()
            .map(NavItem::compose_badge)
            .collect::<Vec<_>>(),
        selected,
        on_select: Rc::new(on_select),
        size: NavBarSize::default(),
        label_behavior: NavBarLabelBehavior::default(),
        indicator_style: NavBarIndicatorStyle::default(),
        safe_area: true,
    }
}

/// PascalCase alias for [`navigation_bar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn NavigationBar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<NavItem<State>>,
    selected: usize,
    on_select: F,
) -> NavigationBarView<State> {
    navigation_bar(items, selected, on_select)
}

impl<State: 'static> NavigationBarView<State> {
    /// Container height variant (`M3ENavigationBar.size`).
    pub fn size(mut self, size: NavBarSize) -> Self {
        self.size = size;
        self
    }

    /// When destination labels are painted
    /// (`M3ENavigationBar.labelBehavior`).
    pub fn label_behavior(mut self, behavior: NavBarLabelBehavior) -> Self {
        self.label_behavior = behavior;
        self
    }

    /// Selection indicator style (`M3ENavigationBar.indicatorStyle`).
    pub fn indicator_style(mut self, style: NavBarIndicatorStyle) -> Self {
        self.indicator_style = style;
        self
    }

    /// Whether the bar consumes the bottom window inset itself, self-sizing
    /// around it and painting its container fill through it — matching Flutter's
    /// Material 3 NavigationBar (which reads `MediaQuery.padding.bottom` only;
    /// horizontals belong to the Scaffold). Default `true`. The bar consumes the
    /// **bottom** inset **only**; left/right display-cutout insets are the
    /// caller's responsibility.
    ///
    /// For a bottom-docked bar respecting horizontal safe areas / landscape
    /// cutouts, wrap the bar in `safe_area(bar).top(false).bottom(false)`
    /// — the bottom-false leaves the bar's self-inset unconsumed by the safe
    /// area, so the bar still paints under its gesture area and self-paints
    /// the cutout band. For a bar embedded mid-screen (e.g. in a preview or
    /// gallery), use `.safe_area(false)`.
    ///
    /// (`M3ENavigationBar.safeArea`)
    pub fn safe_area(mut self, enabled: bool) -> Self {
        self.safe_area = enabled;
        self
    }
}

/// Erase `on_select` into a per-item callback that always reports `idx`
/// (mirrors [`frust::authoring::erase_callback_arg`], but with the index closed over
/// rather than passed at call time — every item needs its *own* fixed index).
fn item_on_select<State: 'static>(
    on_select: &OnSelect<State>,
    idx: usize,
) -> frust::authoring::ErasedCallback {
    let callback = on_select.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state, idx);
    })
}

/// One destination's retained content — a plain [`Widget`] (not
/// `View`/[`AnyView`]-erased; see the [module docs](self)) wrapped in a
/// [`ChildPod`] by [`NavigationBarWidget`] purely to share the multi-child
/// routing helpers.
struct NavItemWidget {
    icon: Option<ChildPod>,
    /// Present only while this destination paints its label (see
    /// [`label_shown`]).
    label: Option<ChildPod>,
    /// The label text, retained for the semantics node's label (which carries
    /// it whether or not the label is painted).
    label_text: String,
    selected: bool,
    /// The newly-selected icon's scale pop; parked at its settled value except
    /// while an incoming selection is springing (see [`ICON_POP_SPRING`]).
    icon_pop: AnimationController,
    on_select: frust::authoring::ErasedCallback,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` — see [`frust::button`]'s
    /// identical fire-on-up-inside contract. There is no `pressed` visual to
    /// track alongside it: the reference paints no ink splash here.
    captured: bool,
    /// This destination's indicator box, in item-local px, resolved in
    /// `layout`. [`NavigationBarWidget`] reads the selected item's copy to
    /// place every indicator style.
    indicator_rect: Rect,
}

/// Build one item's retained [`ChildPod`] (wrapping a fresh [`NavItemWidget`]).
fn build_item<State: 'static>(
    item: &NavItem<State>,
    selected: bool,
    behavior: NavBarLabelBehavior,
    on_select: &OnSelect<State>,
    idx: usize,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let icon = item
        .icon
        .as_ref()
        .map(|icon| frust::authoring::build_child(icon, ctx));
    let label = label_shown(behavior, selected).then(|| {
        let view = label_view::<State>(item.label.clone(), label_role(selected));
        frust::authoring::build_child(&view, ctx)
    });

    let widget = NavItemWidget {
        icon,
        label,
        label_text: item.label.clone(),
        selected,
        // No pop on first mount: only a *change* of selection pops
        // (`_M3ENavIconScaleState.didUpdateWidget`).
        icon_pop: settled_pop(),
        on_select: item_on_select::<State>(on_select, idx),
        captured: false,
        indicator_rect: Rect::ZERO,
    };
    ChildPod::new(Box::new(widget))
}

impl Widget for NavItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // The bar always constrains an item tightly to its own height; an
        // unbounded parent falls back to the default container height.
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            NavBarSize::default().height()
        };

        let label_size = self.label.as_mut().map(|label| {
            label.layout_child(ctx, &BoxConstraints::loose(Size::new(width, f64::INFINITY)))
        });
        // The reference centres the icon-plus-label column inside the bar's
        // height (`Column(mainAxisAlignment: center)`), so a label-less
        // destination centres its indicator box on its own.
        let content_h = INDICATOR_H + label_size.map_or(0.0, |size| LABEL_GAP + size.height);
        let top = ((height - content_h) / 2.0).max(0.0);

        let pill_w = INDICATOR_W.min(width);
        let pill_x = (width - pill_w) / 2.0;
        self.indicator_rect = Rect::new(pill_x, top, pill_x + pill_w, top + INDICATOR_H);

        if let Some(icon) = self.icon.as_mut() {
            let icon_size =
                icon.layout_child(ctx, &BoxConstraints::loose(Size::new(pill_w, INDICATOR_H)));
            icon.set_origin(Point::new(
                pill_x + (pill_w - icon_size.width) / 2.0,
                top + (INDICATOR_H - icon_size.height) / 2.0,
            ));
        }
        if let (Some(label), Some(size)) = (self.label.as_mut(), label_size) {
            label.set_origin(Point::new(
                (width - size.width) / 2.0,
                top + INDICATOR_H + LABEL_GAP,
            ));
        }
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let reduce_motion = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        if reduce_motion {
            // A scale pop has no reduced-motion analogue: land it and stop
            // asking for frames.
            if self.icon_pop.is_animating() {
                self.icon_pop = settled_pop();
            }
        } else if self.icon_pop.advance(ctx.frame_time()) {
            ctx.request_frame();
        }

        if let Some(icon) = self.icon.as_mut() {
            let scale = ICON_POP_FROM + (1.0 - ICON_POP_FROM) * self.icon_pop.value();
            let popped = (scale - 1.0).abs() > ICON_POP_EPSILON;
            if popped {
                let center = (ctx.origin() + icon.origin().to_vec2()).to_vec2()
                    + icon.size().to_vec2() / 2.0;
                scene.push_transform(
                    Affine::translate(center) * Affine::scale(scale) * Affine::translate(-center),
                );
            }
            icon.paint_child(ctx, scene);
            if popped {
                scene.pop_transform();
            }
        }
        if let Some(label) = self.label.as_mut() {
            label.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (self.on_select)(ctx);
                }
                self.captured = false;
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Tab, |node| {
            node.set_label(self.label_text.as_str());
            node.set_selected(self.selected);
        });
    }

    frust::authoring::visit_children!(icon, label);
}

/// Synthesize a [`PointerPhase::Cancel`] into a still-armed item pod (mirrors
/// [`crate`]'s crate-private `cancel_pod`, reimplemented locally since a
/// removed item's `NavItemWidget` isn't `AnyView`-wrapped and so can't go
/// through [`frust::authoring::teardown_child`]'s `Box<dyn Widget>` downcast).
fn cancel_item(pod: &mut ChildPod) {
    let mut dummy_state = ();
    let mut ctx = EventCtx::new(&mut dummy_state, pod.origin(), pod.size());
    let cancel = InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Cancel,
        position: Point::ZERO,
        button: PointerButton::Primary,
    });
    pod.event_child(&mut ctx, &cancel);
}

/// Tear down one item's icon and label children (the shared tail of a
/// truncating rebuild and of `View::teardown`).
fn teardown_item<State: 'static>(
    item: &NavItem<State>,
    selected: bool,
    behavior: NavBarLabelBehavior,
    widget: &mut NavItemWidget,
    ctx: &mut BuildCtx<'_>,
) {
    if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
        frust::authoring::teardown_child(icon_view, icon_pod, ctx);
    }
    if let (true, Some(label_pod)) = (label_shown(behavior, selected), widget.label.as_mut()) {
        let view = label_view::<State>(item.label.clone(), label_role(selected));
        frust::authoring::teardown_child(&view, label_pod, ctx);
    }
}

/// The retained widget for a [`NavigationBarView`].
pub struct NavigationBarWidget {
    /// Each destination, in order — every pod wraps a [`NavItemWidget`].
    items: Vec<ChildPod>,
    /// The app-confirmed selection (see the [module docs](self)' controlled
    /// contract).
    selected: usize,
    size: NavBarSize,
    safe_area: bool,
    /// The bottom window inset consumed on the last `layout` pass — `0.0`
    /// when `safe_area` is disabled or no shell pushed a nonzero inset. Kept
    /// so `paint`/tests can read what `layout` consumed without recomputing
    /// it against a live `LayoutCtx`.
    bottom_inset: f64,
    indicator_style: NavBarIndicatorStyle,
    indicator: LiquidIndicator,
    /// Raised by `rebuild` when the selection moved, consumed by the next
    /// `layout` — which is where the destination's geometry is known and the
    /// travel therefore starts.
    selection_moved: bool,
}

impl<State: 'static> View<State> for NavigationBarView<State> {
    type Element = NavigationBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NavigationBarWidget {
        let mut items = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.iter().enumerate() {
            items.push(build_item(
                item,
                i == self.selected,
                self.label_behavior,
                &self.on_select,
                i,
                ctx,
            ));
        }
        NavigationBarWidget {
            items,
            selected: self.selected,
            size: self.size,
            safe_area: self.safe_area,
            bottom_inset: 0.0,
            indicator_style: self.indicator_style,
            indicator: LiquidIndicator::new(),
            selection_moved: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut NavigationBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let common = prev.items.len().min(self.items.len());

        for i in 0..common {
            let prev_item = &prev.items[i];
            let next_item = &self.items[i];
            let was_selected = i == prev.selected;
            let now_selected = i == self.selected;
            let pod = &mut element.items[i];
            let widget = pod
                .widget_mut()
                .downcast_mut::<NavItemWidget>()
                .expect("nav item pod holds a NavItemWidget");

            match (&prev_item.icon, &next_item.icon) {
                (Some(p), Some(n)) => {
                    flags |= frust::authoring::rebuild_child(
                        p,
                        n,
                        widget.icon.as_mut().expect("icon pod present"),
                        ctx,
                    );
                }
                (None, Some(n)) => {
                    widget.icon = Some(frust::authoring::build_child(n, ctx));
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (Some(p), None) => {
                    if let Some(mut old) = widget.icon.take() {
                        frust::authoring::teardown_child(p, &mut old, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (None, None) => {}
            }

            let prev_role = label_role(was_selected);
            let next_role = label_role(now_selected);
            let was_shown = label_shown(prev.label_behavior, was_selected);
            let now_shown = label_shown(self.label_behavior, now_selected);
            widget.label_text = next_item.label.clone();
            match (was_shown, now_shown) {
                (true, true) => {
                    if (prev_item.label != next_item.label || prev_role != next_role)
                        && let Some(label) = widget.label.as_mut()
                    {
                        let prev_view = label_view::<State>(prev_item.label.clone(), prev_role);
                        let next_view = label_view::<State>(next_item.label.clone(), next_role);
                        flags |=
                            frust::authoring::rebuild_child(&prev_view, &next_view, label, ctx);
                    }
                }
                (false, true) => {
                    let view = label_view::<State>(next_item.label.clone(), next_role);
                    widget.label = Some(frust::authoring::build_child(&view, ctx));
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (true, false) => {
                    if let Some(mut label) = widget.label.take() {
                        let view = label_view::<State>(prev_item.label.clone(), prev_role);
                        frust::authoring::teardown_child(&view, &mut label, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (false, false) => {}
            }

            if was_selected != now_selected {
                widget.selected = now_selected;
                if now_selected {
                    widget.icon_pop = launched_pop();
                } else {
                    widget.icon_pop = settled_pop();
                }
                flags |= ChangeFlags::PAINT;
            }

            // Closures aren't comparable — always reinstall the adapter.
            widget.on_select = item_on_select::<State>(&self.on_select, i);
        }

        if self.items.len() > prev.items.len() {
            for (offset, item) in self.items[common..].iter().enumerate() {
                let idx = common + offset;
                element.items.push(build_item(
                    item,
                    idx == self.selected,
                    self.label_behavior,
                    &self.on_select,
                    idx,
                    ctx,
                ));
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if self.items.len() < prev.items.len() {
            for (offset, item) in prev.items[common..].iter().enumerate() {
                let idx = common + offset;
                let pod = &mut element.items[idx];
                if pod.is_active() {
                    cancel_item(pod);
                    pod.set_active(false);
                }
                let widget = pod
                    .widget_mut()
                    .downcast_mut::<NavItemWidget>()
                    .expect("nav item pod holds a NavItemWidget");
                teardown_item(item, idx == prev.selected, prev.label_behavior, widget, ctx);
            }
            element.items.truncate(common);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.selected != self.selected {
            element.selected = self.selected;
            // The indicator's travel starts in `layout` (the pass that knows
            // where the destination *is*), so a moved selection must ask for
            // one — a bare repaint would leave the pill parked.
            element.selection_moved = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.safe_area != self.safe_area {
            element.safe_area = self.safe_area;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.label_behavior != self.label_behavior {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.indicator_style != self.indicator_style {
            element.indicator_style = self.indicator_style;
            flags |= ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut NavigationBarWidget, ctx: &mut BuildCtx<'_>) {
        for (i, (item, pod)) in self.items.iter().zip(element.items.iter_mut()).enumerate() {
            let widget = pod
                .widget_mut()
                .downcast_mut::<NavItemWidget>()
                .expect("nav item pod holds a NavItemWidget");
            teardown_item(item, i == self.selected, self.label_behavior, widget, ctx);
        }
    }
}

impl NavigationBarWidget {
    /// The selected destination's indicator box in bar-local px, or `None`
    /// when the bar holds no items.
    fn selected_indicator_rect(&mut self) -> Option<Rect> {
        let last = self.items.len().checked_sub(1)?;
        let index = self.selected.min(last);
        let pod = self.items.get_mut(index)?;
        let origin = pod.origin();
        let local = pod
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .expect("nav item pod holds a NavItemWidget")
            .indicator_rect;
        Some(local + origin.to_vec2())
    }
}

impl Widget for NavigationBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let band = self.size.height();
        let inset = if self.safe_area {
            ctx.window_insets().padding().bottom
        } else {
            0.0
        };
        self.bottom_inset = inset;
        let height = band + inset;
        // Reduced motion turns the travel into the same jump every non-selection
        // geometry change takes.
        let reduce_motion = Theme::from_layout_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let travel = std::mem::take(&mut self.selection_moved)
            && self.indicator_style == NavBarIndicatorStyle::Pill
            && !reduce_motion;

        let n = self.items.len();
        if n == 0 {
            return bc.constrain(Size::new(width, height));
        }
        let slot_w = width / n as f64;
        for (i, pod) in self.items.iter_mut().enumerate() {
            pod.layout_child(ctx, &BoxConstraints::tight(Size::new(slot_w, band)));
            pod.set_origin(Point::new(i as f64 * slot_w, 0.0));
        }
        if let Some(rest) = self.selected_indicator_rect() {
            self.indicator.sync(rest, travel);
        }
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (container_fill, indicator_fill) = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        scene.fill_rect(ctx.origin(), ctx.size(), container_fill);

        match self.indicator_style {
            NavBarIndicatorStyle::Pill => {
                if reduce_motion {
                    self.indicator.settle();
                } else if self.indicator.advance(ctx.frame_time()) {
                    // The pill is paint-only geometry: no item's layout depends
                    // on it, so a plain frame request is enough.
                    ctx.request_frame();
                }
                if let Some(pill) = self.indicator.pill_rect() {
                    scene.fill_rounded_rect(
                        ctx.origin() + Vec2::new(pill.x0, pill.y0),
                        pill.size(),
                        self.indicator.radius(),
                        indicator_fill,
                    );
                }
            }
            NavBarIndicatorStyle::Underline => {
                if let Some(rest) = self.selected_indicator_rect() {
                    // A `Border(bottom:)` paints inside its box's bottom edge.
                    scene.fill_rect(
                        ctx.origin() + Vec2::new(rest.x0, rest.y1 - UNDERLINE_THICKNESS),
                        Size::new(rest.width(), UNDERLINE_THICKNESS),
                        indicator_fill,
                    );
                }
            }
            NavBarIndicatorStyle::None => {}
        }

        for pod in &mut self.items {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.items, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BuildCtx, PointerButton, PointerEvent, WindowEdgeInsets, WindowInsets};
    use frust_widgets::test_support::{RecordingScene, leaf_any};
    use std::any::Any;
    use std::cell::Cell;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build(view: &NavigationBarView<()>) -> NavigationBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    fn layout(w: &mut NavigationBarWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, bc)
    }

    fn layout_themed(w: &mut NavigationBarWidget, bc: &BoxConstraints, theme: &Theme) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(theme as &dyn Any));
        w.layout(&mut lctx, bc)
    }

    /// Lay out under a window carrying `bottom` px of bottom system-bar inset
    /// and nothing else, the shape a shell pushes for a home-indicator/nav-bar
    /// occlusion.
    fn layout_with_bottom_inset(
        w: &mut NavigationBarWidget,
        bc: &BoxConstraints,
        bottom: f64,
    ) -> Size {
        let insets = WindowInsets::new(
            WindowEdgeInsets::new(0.0, 0.0, 0.0, bottom),
            WindowEdgeInsets::ZERO,
        );
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        lctx.with_window_insets(insets, |ctx| w.layout(ctx, bc))
    }

    fn three_items() -> Vec<NavItem<()>> {
        vec![nav_item("Home"), nav_item("Search"), nav_item("Profile")]
    }

    fn ft(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// Records the plain fills, rounded fills, and transforms a paint pass
    /// emits.
    #[derive(Default)]
    struct PillRecorder {
        rects: Vec<(Point, Size)>,
        rounded: Vec<(Point, Size, f64)>,
        transforms: Vec<Affine>,
        transform_pops: u32,
    }

    impl PaintScene for PillRecorder {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, _color: Color) {
            self.rounded.push((origin, size, radius));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    fn paint_at(w: &mut NavigationBarWidget, size: Size, time: FrameTime) -> (PillRecorder, bool) {
        let mut scene = PillRecorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, time);
        w.paint(&mut pctx, &mut scene);
        let needs_frame = pctx.needs_frame();
        (scene, needs_frame)
    }

    fn paint_themed_at(
        w: &mut NavigationBarWidget,
        size: Size,
        time: FrameTime,
        theme: &Theme,
    ) -> (PillRecorder, bool) {
        let mut scene = PillRecorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, time).with_theme(theme);
        w.paint(&mut pctx, &mut scene);
        let needs_frame = pctx.needs_frame();
        (scene, needs_frame)
    }

    // ---- Layout / sizes / label behaviors ---------------------------------

    #[test]
    fn layout_divides_width_evenly_and_fills_height() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, NavBarSize::default().height()));
        assert_eq!(w.items[0].origin().x, 0.0);
        assert_eq!(w.items[1].origin().x, 100.0);
        assert_eq!(w.items[2].origin().x, 200.0);
        assert_eq!(
            w.items[0].size(),
            Size::new(100.0, NavBarSize::default().height())
        );
    }

    #[test]
    fn the_two_sizes_carry_their_reference_container_heights() {
        assert_eq!(NavBarSize::Small.height(), HEIGHT_SMALL);
        assert_eq!(NavBarSize::Medium.height(), HEIGHT_MEDIUM);
        for (size, expected) in [
            (NavBarSize::Small, HEIGHT_SMALL),
            (NavBarSize::Medium, HEIGHT_MEDIUM),
        ] {
            let view: NavigationBarView<()> =
                navigation_bar(three_items(), 0, |_s: &mut (), _i| {}).size(size);
            let mut w = build(&view);
            let laid = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
            assert_eq!(laid.height, expected, "{size:?}");
            assert_eq!(w.items[0].size().height, expected, "{size:?}");
        }
    }

    #[test]
    fn default_size_is_medium() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, HEIGHT_MEDIUM));
        assert_eq!(w.items[0].size().height, HEIGHT_MEDIUM);
    }

    // ---- Self-inset (`safe_area`) ------------------------------------------

    #[test]
    fn safe_area_grows_the_bar_by_the_consumed_bottom_inset() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let size = layout_with_bottom_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            34.0,
        );
        assert_eq!(
            size,
            Size::new(300.0, NavBarSize::default().height() + 34.0)
        );
        assert_eq!(w.bottom_inset, 34.0);
    }

    #[test]
    fn items_stay_at_the_bare_band_height_under_a_self_consumed_inset() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout_with_bottom_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            34.0,
        );
        for item in &w.items {
            assert_eq!(
                item.origin().y,
                0.0,
                "items sit at the top of the taller bar"
            );
            assert_eq!(
                item.size().height,
                NavBarSize::default().height(),
                "items stay the bare band tall"
            );
        }
    }

    #[test]
    fn indicator_geometry_is_unchanged_by_a_self_consumed_inset() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 1, |_s: &mut (), _i| {});
        let bc = BoxConstraints::loose(Size::new(300.0, 200.0));

        let mut zero = build(&view);
        layout(&mut zero, &bc);
        let (zero_scene, _) = paint_at(
            &mut zero,
            Size::new(300.0, NavBarSize::default().height()),
            ft(0.0),
        );

        let mut inset = build(&view);
        layout_with_bottom_inset(&mut inset, &bc, 34.0);
        let (inset_scene, _) = paint_at(
            &mut inset,
            Size::new(300.0, NavBarSize::default().height() + 34.0),
            ft(0.0),
        );

        assert_eq!(
            zero_scene.rounded, inset_scene.rounded,
            "the pill's geometry is item-local and byte-identical for a nonzero inset"
        );
    }

    #[test]
    fn safe_area_false_ignores_the_window_inset() {
        let view: NavigationBarView<()> =
            navigation_bar(three_items(), 0, |_s: &mut (), _i| {}).safe_area(false);
        let mut w = build(&view);
        let size = layout_with_bottom_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            34.0,
        );
        assert_eq!(size, Size::new(300.0, NavBarSize::default().height()));
        assert_eq!(w.bottom_inset, 0.0, "an opted-out bar consumes nothing");
    }

    #[test]
    fn container_fill_covers_the_full_band_plus_inset_size() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout_with_bottom_inset(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            34.0,
        );
        let (scene, _) = paint_at(
            &mut w,
            Size::new(300.0, NavBarSize::default().height() + 34.0),
            ft(0.0),
        );
        assert_eq!(
            scene.rects[0],
            (
                Point::ZERO,
                Size::new(300.0, NavBarSize::default().height() + 34.0)
            ),
            "the container fill covers the band plus the consumed inset"
        );
    }

    #[test]
    fn rebuild_flipping_safe_area_returns_layout() {
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));

        let next: NavigationBarView<()> =
            navigation_bar(three_items(), 0, |_s: &mut (), _i| {}).safe_area(false);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(
            flags.needs_layout(),
            "safe_area toggles the self-inset, a layout change"
        );
        assert!(!w.safe_area);
    }

    #[test]
    fn label_behavior_decides_which_destinations_hold_a_label_child() {
        let cases = [
            (NavBarLabelBehavior::AlwaysShow, [true, true, true]),
            (NavBarLabelBehavior::OnlySelected, [false, true, false]),
            (NavBarLabelBehavior::AlwaysHide, [false, false, false]),
        ];
        for (behavior, expected) in cases {
            let view: NavigationBarView<()> =
                navigation_bar(three_items(), 1, |_s: &mut (), _i| {}).label_behavior(behavior);
            let mut w = build(&view);
            layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
            for (i, want) in expected.iter().enumerate() {
                let item = w.items[i]
                    .widget_mut()
                    .downcast_mut::<NavItemWidget>()
                    .unwrap();
                assert_eq!(item.label.is_some(), *want, "{behavior:?} item {i}");
                // The semantics label survives a hidden label either way.
                assert!(!item.label_text.is_empty());
            }
        }
    }

    #[test]
    fn a_hidden_label_centres_the_indicator_box_on_its_own() {
        let hidden: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {})
            .label_behavior(NavBarLabelBehavior::AlwaysHide);
        let mut w = build(&hidden);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let item = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        let expected_top = (NavBarSize::default().height() - INDICATOR_H) / 2.0;
        assert!((item.indicator_rect.y0 - expected_top).abs() < 1e-9);
        assert_eq!(item.indicator_rect.width(), INDICATOR_W);
    }

    #[test]
    fn changing_the_label_behavior_asks_for_a_relayout() {
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let next: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {})
            .label_behavior(NavBarLabelBehavior::AlwaysHide);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout(), "label presence is a layout input");
        let item = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!(item.label.is_none(), "the label child was torn down");
    }

    // ---- Indicator styles --------------------------------------------------

    #[test]
    fn the_three_indicator_styles_paint_a_pill_an_underline_or_nothing() {
        let bar = Size::new(300.0, HEIGHT_SMALL);

        let pill: NavigationBarView<()> = navigation_bar(three_items(), 1, |_s: &mut (), _i| {});
        let mut w = build(&pill);
        layout(&mut w, &BoxConstraints::loose(bar));
        let (scene, _) = paint_at(&mut w, bar, ft(0.0));
        assert_eq!(scene.rects.len(), 1, "container fill only");
        assert_eq!(scene.rounded.len(), 1, "one stadium pill");
        let (origin, size, radius) = scene.rounded[0];
        assert_eq!(size, Size::new(INDICATOR_W, INDICATOR_H));
        assert_eq!(radius, INDICATOR_H / 2.0);
        // Item 1 spans x in [100, 200): its pill is centred at 150.
        assert!((origin.x + size.width / 2.0 - 150.0).abs() < 1e-9);

        let underline: NavigationBarView<()> =
            navigation_bar(three_items(), 1, |_s: &mut (), _i| {})
                .indicator_style(NavBarIndicatorStyle::Underline);
        let mut w = build(&underline);
        layout(&mut w, &BoxConstraints::loose(bar));
        let (scene, _) = paint_at(&mut w, bar, ft(0.0));
        assert!(scene.rounded.is_empty(), "no pill in underline style");
        assert_eq!(scene.rects.len(), 2, "container plus underline");
        let (origin, size) = scene.rects[1];
        assert_eq!(size, Size::new(INDICATOR_W, UNDERLINE_THICKNESS));
        assert!((origin.x + size.width / 2.0 - 150.0).abs() < 1e-9);

        let none: NavigationBarView<()> = navigation_bar(three_items(), 1, |_s: &mut (), _i| {})
            .indicator_style(NavBarIndicatorStyle::None);
        let mut w = build(&none);
        layout(&mut w, &BoxConstraints::loose(bar));
        let (scene, needs_frame) = paint_at(&mut w, bar, ft(0.0));
        assert!(scene.rounded.is_empty());
        assert_eq!(scene.rects.len(), 1, "container fill only");
        assert!(!needs_frame);
    }

    // ---- The liquid indicator ---------------------------------------------

    #[test]
    fn the_edge_springs_match_the_reference_preset_with_only_damping_overridden() {
        let reference = crate::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT;
        assert_eq!(LEAD_SPRING.stiffness, reference.stiffness);
        assert_eq!(TRAIL_SPRING.stiffness, reference.stiffness);
        assert_eq!(ICON_POP_SPRING.stiffness, reference.stiffness);
        assert_eq!(LEAD_SPRING.mass, 1.0);
        assert_eq!(TRAIL_SPRING.mass, 1.0);
        assert_eq!(LEAD_SPRING.damping_ratio, 0.45);
        assert_eq!(TRAIL_SPRING.damping_ratio, 0.55);
        assert_eq!(ICON_POP_SPRING.damping_ratio, 0.5);
        // The ordering is what makes the bridge stretch at all: the lead edge
        // must be the less damped of the two.
        const {
            assert!(LEAD_SPRING.damping_ratio < TRAIL_SPRING.damping_ratio);
        }
    }

    /// The two-phase timeline: a selection change stretches the pill into a
    /// bridge between the two destinations, then settles it onto the new one.
    #[test]
    fn the_pill_stretches_toward_the_new_destination_and_then_settles() {
        let bar = Size::new(300.0, HEIGHT_SMALL);
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(bar));
        let (scene, needs_frame) = paint_at(&mut w, bar, ft(0.0));
        assert_eq!(scene.rounded[0].1.width, INDICATOR_W, "resting at item 0");
        assert!(!needs_frame, "a settled pill asks for no frames");

        let next: NavigationBarView<()> = navigation_bar(three_items(), 2, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(bar));
        assert!(w.indicator.is_animating(), "the travel started at layout");

        let mut widest: f64 = 0.0;
        let mut widest_at = 0usize;
        let mut lead_ahead = false;
        let mut settled_at = None;
        for frame in 0..240 {
            let (scene, needs_frame) = paint_at(&mut w, bar, ft(16.0 * frame as f64));
            let (origin, size, _) = scene.rounded[0];
            if size.width > widest {
                widest = size.width;
                widest_at = frame;
            }
            // Travelling right: the lead edge is the right one, so the pill's
            // right edge passes the destination's own before the left catches
            // up.
            if origin.x + size.width > 250.0 + INDICATOR_W / 2.0 - 1.0 && size.width > INDICATOR_W {
                lead_ahead = true;
            }
            if !needs_frame {
                settled_at = Some(frame);
                break;
            }
        }
        assert!(
            widest > INDICATOR_W + 1.0,
            "phase 1 never stretched: widest {widest} vs resting {INDICATOR_W}"
        );
        assert!(widest_at > 0, "the stretch builds up over frames");
        assert!(lead_ahead, "the leading edge reaches the new slot first");

        let settled_at = settled_at.expect("the pill settles within 240 frames");
        assert!(settled_at > widest_at, "settling follows the stretch");
        let (scene, needs_frame) = paint_at(&mut w, bar, ft(16.0 * (settled_at + 1) as f64));
        assert!(!needs_frame, "a settled pill stops asking for frames");
        let (origin, size, _) = scene.rounded[0];
        assert!(
            (size.width - INDICATOR_W).abs() < 1e-6,
            "phase 2 must collapse the bridge back to the resting width"
        );
        // Item 2 spans x in [200, 300): its pill is centred at 250.
        assert!((origin.x + size.width / 2.0 - 250.0).abs() < 1e-6);
    }

    /// `layout` re-enters on every full-tree layout pass, not just the one
    /// where the selection actually moved — an animating sibling's own
    /// `request_layout`, or any other app-side LAYOUT flag, produces exactly
    /// this shape: a relayout that reproduces the *same* resting geometry
    /// while a travel is already mid-flight. It must not snap the pill.
    #[test]
    fn a_same_geometry_relayout_mid_travel_does_not_snap_the_pill() {
        let bar = Size::new(300.0, HEIGHT_SMALL);
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(bar));
        paint_at(&mut w, bar, ft(0.0));

        let next: NavigationBarView<()> = navigation_bar(three_items(), 2, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(bar));
        assert!(w.indicator.is_animating(), "the travel started at layout");

        // Run a few paint frames so the pill is genuinely mid-stretch, not
        // just launched.
        paint_at(&mut w, bar, ft(16.0));
        paint_at(&mut w, bar, ft(32.0));
        let (scene, _) = paint_at(&mut w, bar, ft(48.0));
        let mid_flight_width = scene.rounded[0].1.width;
        assert!(
            mid_flight_width > INDICATOR_W + 1.0,
            "must actually be stretched before the interleaved layout, got {mid_flight_width}"
        );

        // An unrelated relayout — same items, same selection, same
        // constraints — reproduces the exact same resting rect. Pre-fix,
        // `sync`'s non-travel branch snapped unconditionally here.
        layout(&mut w, &BoxConstraints::loose(bar));
        assert!(
            w.indicator.is_animating(),
            "a same-geometry relayout mid-travel must not cancel the travel"
        );
        let (scene, _) = paint_at(&mut w, bar, ft(64.0));
        let width_after = scene.rounded[0].1.width;
        assert!(
            width_after > INDICATOR_W + 1.0,
            "the pill must still be mid-flight after the interleaved layout, \
             not snapped to rest (got {width_after})"
        );

        // The interrupted travel still settles correctly afterward.
        let mut settled = false;
        for frame in 5..240 {
            let (scene, needs_frame) = paint_at(&mut w, bar, ft(16.0 * frame as f64));
            if !needs_frame {
                let (origin, size, _) = scene.rounded[0];
                assert!((size.width - INDICATOR_W).abs() < 1e-6);
                // Item 2 spans x in [200, 300): its pill is centred at 250.
                assert!((origin.x + size.width / 2.0 - 250.0).abs() < 1e-6);
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "the pill still settles within 240 frames despite the interleave"
        );
    }

    #[test]
    fn a_width_change_jumps_the_pill_instead_of_travelling() {
        let mut counter = 0u64;
        let view: NavigationBarView<()> = navigation_bar(three_items(), 1, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        layout(&mut w, &BoxConstraints::loose(Size::new(600.0, 200.0)));
        assert!(!w.indicator.is_animating(), "a relayout never travels");
        let pill = w.indicator.pill_rect().expect("laid out");
        assert!((pill.width() - INDICATOR_W).abs() < 1e-9);
        // Item 1 now spans x in [200, 400): centred at 300.
        assert!((pill.center().x - 300.0).abs() < 1e-9);
    }

    #[test]
    fn reduce_motion_snaps_the_liquid_travel_and_asks_for_no_frames() {
        let bar = Size::new(300.0, HEIGHT_SMALL);
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        layout_themed(&mut w, &BoxConstraints::loose(bar), &theme);

        let next: NavigationBarView<()> = navigation_bar(three_items(), 2, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        layout_themed(&mut w, &BoxConstraints::loose(bar), &theme);
        assert!(!w.indicator.is_animating(), "reduce_motion never travels");

        let (scene, needs_frame) = paint_themed_at(&mut w, bar, ft(0.0), &theme);
        assert!(!needs_frame, "reduce_motion asks for no continuation frame");
        let (origin, size, _) = scene.rounded[0];
        assert!((size.width - INDICATOR_W).abs() < 1e-9, "no stretch at all");
        assert!((origin.x + size.width / 2.0 - 250.0).abs() < 1e-9);
    }

    // ---- Icon pop ----------------------------------------------------------

    #[test]
    fn selecting_an_item_pops_its_icon_and_then_returns_to_rest() {
        let bar = Size::new(300.0, HEIGHT_SMALL);
        let mut counter = 0u64;
        let items = || {
            vec![
                nav_item::<()>("Home").icon(leaf_any(24.0, 24.0)),
                nav_item::<()>("Search").icon(leaf_any(24.0, 24.0)),
            ]
        };
        let prev: NavigationBarView<()> = navigation_bar(items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(bar));
        let (scene, _) = paint_at(&mut w, bar, ft(0.0));
        assert!(
            scene.transforms.is_empty(),
            "a settled bar pushes no scale transform"
        );

        let next: NavigationBarView<()> = navigation_bar(items(), 1, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        let item = w.items[1]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!(item.icon_pop.is_animating(), "the incoming icon pops");
        assert!(
            (item.icon_pop.value() - 0.0).abs() < 1e-9,
            "starts at 0.98x"
        );

        layout(&mut w, &BoxConstraints::loose(bar));
        let mut settled = false;
        let mut scaled_frames = 0;
        let mut overshot = false;
        for frame in 0..240 {
            let (scene, needs_frame) = paint_at(&mut w, bar, ft(16.0 * frame as f64));
            assert_eq!(
                scene.transforms.len() as u32,
                scene.transform_pops,
                "every pushed transform is popped"
            );
            if let Some(t) = scene.transforms.first() {
                scaled_frames += 1;
                // A uniform scale's determinant is s².
                if t.determinant() > 1.0 {
                    overshot = true;
                }
            }
            if !needs_frame {
                settled = true;
                break;
            }
        }
        assert!(settled, "the pop settles");
        assert!(scaled_frames > 0, "the pop actually scaled the icon");
        assert!(overshot, "the pop overshoots past the resting scale");
        let item = w.items[1]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!((item.icon_pop.value() - 1.0).abs() < 1e-9, "rests at 1.0x");
    }

    // ---- Badges ------------------------------------------------------------

    #[test]
    fn a_badge_wraps_the_icon_without_moving_the_hit_target() {
        let bar = Size::new(300.0, NavBarSize::default().height());
        let bare: NavigationBarView<()> = navigation_bar(
            vec![nav_item("Home").icon(leaf_any(24.0, 24.0))],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = build(&bare);
        layout(&mut w, &BoxConstraints::loose(bar));
        let bare_icon = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap()
            .icon
            .as_ref()
            .expect("icon pod present")
            .size();
        assert_eq!(bare_icon, Size::new(24.0, 24.0));

        let tapped: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
        let sink = tapped.clone();
        let badged: NavigationBarView<()> = navigation_bar(
            vec![
                nav_item("Home").icon(leaf_any(24.0, 24.0)).badge_count(3),
                nav_item("Search").icon(leaf_any(24.0, 24.0)).badge_dot(),
            ],
            0,
            move |_s: &mut (), i| sink.set(Some(i)),
        );
        let mut w = build(&badged);
        layout(&mut w, &BoxConstraints::loose(bar));
        let item = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        let badged_icon = item.icon.as_ref().expect("icon pod present").size();
        assert!(
            badged_icon.width > bare_icon.width,
            "the badge composes onto the icon: {badged_icon:?}"
        );
        assert!(badged_icon.height <= INDICATOR_H, "still inside the box");
        // The item's own slot — and therefore its hit target — is untouched.
        assert_eq!(
            w.items[1].size(),
            Size::new(150.0, NavBarSize::default().height())
        );

        let mut unit = ();
        let unit_any: &mut dyn Any = &mut unit;
        let mut ectx = EventCtx::new(
            unit_any,
            Point::ZERO,
            Size::new(300.0, NavBarSize::default().height()),
        );
        w.event(&mut ectx, &ev(PointerPhase::Down, 200.0, 20.0));
        w.event(&mut ectx, &ev(PointerPhase::Up, 200.0, 20.0));
        assert_eq!(
            tapped.get(),
            Some(1),
            "a badged bar still reports the tapped index"
        );
    }

    // ---- Colors ------------------------------------------------------------

    #[test]
    fn unthemed_paint_uses_fallback_container() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT_SMALL));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, HEIGHT_SMALL));
    }

    #[test]
    fn themed_paint_resolves_surface_container_and_the_indicator_role() {
        let theme = crate::baseline();
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout_themed(
            &mut w,
            &BoxConstraints::loose(Size::new(300.0, 200.0)),
            &theme,
        );

        #[derive(Default)]
        struct ColorRecorder {
            colors: Vec<Color>,
            rounded: Vec<Color>,
        }
        impl PaintScene for ColorRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, c: Color) {
                self.colors.push(c);
            }
            fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, c: Color) {
                self.rounded.push(c);
            }
            fn draw_text(&mut self, _o: Point, _t: &str) {}
        }
        let mut scene = ColorRecorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, Size::new(300.0, HEIGHT_SMALL), ft(0.0))
            .with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.colors[0], theme.scheme().surface_container);
        assert_eq!(scene.rounded[0], theme.scheme().secondary_container);
    }

    // ---- Controlled semantics ---------------------------------------------

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // `state` must be a concrete `&mut Vec<usize>`: it is erased to `&mut dyn
    // Any` and recovered via `state_mut::<Vec<usize>>()`, so a slice would fail
    // the downcast (mirrors `lib.rs`'s identical `route` helper).
    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut NavigationBarWidget, state: &mut Vec<usize>, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT_SMALL));
        w.event(&mut ctx, event);
    }

    #[test]
    fn tap_inside_an_item_reports_its_index_without_self_mutating() {
        let view: NavigationBarView<Vec<usize>> = navigation_bar(
            vec![nav_item("Home"), nav_item("Search"), nav_item("Profile")],
            0,
            |s: &mut Vec<usize>, i| s.push(i),
        );
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));

        let mut log: Vec<usize> = Vec::new();
        // Item 1 ("Search") occupies x in [100, 200).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 150.0, 20.0));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 150.0, 20.0));
        assert_eq!(log, vec![1]);
        // Controlled: the widget's own selection never moved.
        assert_eq!(w.selected, 0);
        let item = w.items[1]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!(!item.selected, "a tap alone never selects");
    }

    #[test]
    fn rebuild_adopts_new_selected_index() {
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!(widget0.selected);

        let next: NavigationBarView<()> = navigation_bar(three_items(), 2, |_s: &mut (), _i| {});
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_paint());
        assert!(
            flags.needs_layout(),
            "the pill's travel is resolved in layout"
        );

        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!(!widget0.selected, "selection moved off item 0");
        let widget2 = w.items[2]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        assert!(widget2.selected, "item 2 is now selected");
        assert_eq!(w.selected, 2);
        assert!(w.selection_moved, "the next layout starts the travel");
    }

    #[test]
    fn growing_item_list_builds_a_fresh_unselected_item() {
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(
            vec![nav_item("Home"), nav_item("Search")],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        assert_eq!(w.items.len(), 2);

        let next: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.items.len(), 3);
        assert!(flags.needs_layout());
    }

    #[test]
    fn shrinking_item_list_tears_down_the_removed_item() {
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));

        let next: NavigationBarView<()> =
            navigation_bar(vec![nav_item("Home")], 0, |_s: &mut (), _i| {});
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert_eq!(w.items.len(), 1);
        assert!(flags.needs_layout());
    }

    #[test]
    fn a_selection_past_the_last_item_still_places_the_indicator() {
        let mut counter = 0u64;
        let prev: NavigationBarView<()> = navigation_bar(three_items(), 2, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let next: NavigationBarView<()> =
            navigation_bar(vec![nav_item("Home")], 2, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let pill = w.indicator.pill_rect().expect("clamped onto the last item");
        assert!((pill.center().x - 150.0).abs() < 1e-9);
    }

    #[test]
    fn icon_slot_is_routed_and_laid_out_above_the_label() {
        let view: NavigationBarView<()> = navigation_bar(
            vec![nav_item("Home").icon(leaf_any(24.0, 24.0))],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(100.0, 200.0)));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<NavItemWidget>()
            .unwrap();
        let icon = widget0.icon.as_ref().expect("icon pod present");
        let label = widget0.label.as_ref().expect("label pod present");
        assert!(icon.origin().y < label.origin().y);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_state: &mut ()) -> NavigationBarView<()> {
            navigation_bar(three_items(), 1, |_s: &mut (), _i| {})
        }
        let mut root: frust_core::RenderRoot<(), NavigationBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, HEIGHT_SMALL), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let tablist = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TabList)
            .expect("a TabList container node is contributed");
        assert_eq!(tablist.1.children().len(), 3);

        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3);
        let selected_tab = tabs
            .iter()
            .find(|(_, n)| n.label() == Some("Search"))
            .expect("the Search tab is present");
        assert_eq!(selected_tab.1.is_selected(), Some(true));
        let unselected_tab = tabs
            .iter()
            .find(|(_, n)| n.label() == Some("Home"))
            .expect("the Home tab is present");
        assert_eq!(unselected_tab.1.is_selected(), Some(false));
    }

    #[test]
    fn a_hidden_label_still_reaches_the_semantics_node() {
        fn logic(_state: &mut ()) -> NavigationBarView<()> {
            navigation_bar(three_items(), 1, |_s: &mut (), _i| {})
                .label_behavior(NavBarLabelBehavior::AlwaysHide)
        }
        let mut root: frust_core::RenderRoot<(), NavigationBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, HEIGHT_SMALL), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let labelled = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab && n.label().is_some())
            .count();
        assert_eq!(
            labelled, 3,
            "every tab keeps its label with no text painted"
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn every_label_paints_in_roboto_flex_under_the_material_theme() {
        // Selected and unselected labels alike (they differ only in color
        // role) — see `crate::appbar::top::typeface_probe`.
        use crate::appbar::top::typeface_probe::{Face, assert_paints_only_in};
        let flex = crate::tokens::font_data()[0];
        let runs = assert_paints_only_in(
            "the navigation bar's labels",
            |_: &mut ()| navigation_bar(three_items(), 1, |_s: &mut (), _i| {}),
            crate::baseline(),
            &[flex],
            Size::new(300.0, HEIGHT_SMALL),
            Face {
                bytes: flex,
                name: "Roboto Flex",
            },
        );
        assert!(
            runs >= 3,
            "fixture sanity: expected a run per label, got {runs}"
        );
    }
}
