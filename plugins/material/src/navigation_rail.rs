// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/navigation_rail/ — m3e_navigation_rail.dart, its
//   components/ (m3e_rail_item.dart, m3e_rail_item_button.dart,
//   m3e_nav_selection_indicator.dart, m3e_rail_badge_view.dart,
//   m3e_nav_icon_scale.dart, m3e_navigation_rail_children_mixin.dart),
//   enums/, models/, res/m3e_navigation_rail_layout.dart and
//   styles/m3e_navigation_rail_theme.dart (retrieved 2026-08-19). The
//   reference's own tree is in turn vendored from the `navigation_rail_m3e`
//   package (MIT) — already credited in `plugins/material/NOTICE`.
// Porting decisions: expansion is a caller-owned prop here rather than the
// reference's internal `_expanded` mirror; the modal presentation is painted
// in-widget instead of through this crate's `overlay::modal` host; the
// scrollable destination column, the collapsed "peek" overlay, per-item
// tooltips, the icon scale-pop and the Android system-bar scrim styling stay
// unported — see the module docs' *Not ported*.

//! The M3 Expressive **navigation rail**: a vertical destination column that
//! animates between a slim collapsed icon rail and a wide expanded
//! icon-plus-label rail, optionally presenting the expanded state **modally**
//! over a scrim.
//!
//! [`navigation_rail`] is *controlled* twice over, the same never-self-mutating
//! contract every control in this catalog follows
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics):
//!
//! * **Selection** — the caller owns `selected` and receives the pressed
//!   destination's flat index through [`navigation_rail`]'s `on_select`.
//! * **Expansion** — the caller owns [`NavigationRailView::rail_type`] and
//!   receives the *requested* next type through
//!   [`NavigationRailView::on_type_changed`] when the menu button is pressed.
//!   The reference instead keeps a `_expanded` mirror inside its own `State`,
//!   seeds it from `type`, flips it itself and merely *notifies*
//!   `onTypeChanged`; this port drops the mirror so there is exactly one
//!   source of truth (the prop), which is also what makes an app-driven
//!   window-size-class switch (`alwaysCollapse` on compact, `expanded` on
//!   medium+) land without fighting internal state.
//!
//! # Geometry
//!
//! Every value below is `M3ENavigationRailTheme`'s own default
//! (`styles/m3e_navigation_rail_theme.dart`) or a
//! `M3ENavigationRailLayout` constant (`res/`):
//!
//! | Metric | Value |
//! |---|---|
//! | Collapsed width | 96dp ([`RAIL_COLLAPSED_WIDTH`]) — `0` when [`NavigationRailView::hide_when_collapsed`] |
//! | Expanded width | [`NavigationRailView::expanded_width`] clamped to 220–360dp ([`RAIL_EXPANDED_MIN_WIDTH`]/[`RAIL_EXPANDED_MAX_WIDTH`]), defaulting to the 220dp minimum |
//! | Item height | 66dp collapsed, 40dp *minimum* expanded |
//! | Icon | 24dp |
//! | Row insets | 16dp leading/trailing, 8dp icon→label gap, 4dp between items |
//! | Section header | 12dp above, 8dp below, `titleSmall` in `onSurfaceVariant` |
//! | Top gap | 36dp above the menu button |
//!
//! A collapsed destination is a 52×40dp icon chip inside a 48dp tap target —
//! the exact box [`mod@crate::icon_button`] paints for its
//! `IconButtonSize::Sm`/`IconButtonWidth::Wide` pair, which is what the
//! reference builds a collapsed item from — with its label (when
//! [`RailLabelBehavior`] shows one) directly beneath. An expanded destination
//! is a full-width row whose selection indicator *is* the row.
//!
//! # The width transition, and the layout-skip trap
//!
//! The rail's width is a **layout** value driven by an animation, so this is
//! the layout-skip class [`mod@crate::expandable_list`] documents at length: on
//! the mobile intra-frame layout skip
//! (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate`), layout does not re-run
//! merely because paint asked for another frame. While the width is in flight
//! `paint` therefore calls [`frust::authoring::PaintCtx::request_layout`]
//! (which implies `request_frame`), so the next frame relayouts at the
//! freshly-advanced width on every platform, gated or not.
//! `Theme.motion.reduce_motion` snaps straight to the target instead; the snap
//! is still a layout-relevant value change, so it requests exactly one
//! relayout (never an ongoing frame request — the driver is already stopped).
//!
//! **Timing, not a spring**: the reference animates the width with
//! `AnimatedContainer(duration: 280ms, curve: Curves.easeOutCubic)`
//! ([`RAIL_EXPAND_DURATION`], [`RAIL_EXPAND_CURVE`]) — a duration+curve, not a
//! spring — and this port transcribes that rather than substituting a spring
//! token. Flutter's `easeOutCubic` is `cubic-bezier(0.215, 0.61, 0.355, 1.0)`,
//! carried over exactly as a [`frust::Curve::Cubic`]. The *selection*
//! indicator below is the spring-driven part of this widget.
//!
//! # The liquid selection indicator
//!
//! Ported from `components/m3e_nav_selection_indicator.dart`: two independent
//! springs track the **lead** and **trailing** edge centers of the pill along
//! the rail's main axis. The lead spring is less damped ([`RAIL_LEAD_SPRING`],
//! ζ 0.45) than the trailing one ([`RAIL_TRAIL_SPRING`], ζ 0.55), so on a
//! selection change the pill *elongates* into a bridge spanning both
//! destinations and then settles back to a stadium over the newly selected one
//! — the reference's "liquid" morph, on its own `expressiveSpatialDefault`
//! stiffness. Both are read **unclamped** so the lead's overshoot is real
//! travel (the same `crate::switch` spatial-travel convention).
//!
//! While the bridge is traveling the rail paints it and every item's own
//! resting fill is suppressed; once it settles the rail stops painting and each
//! selected item paints its own indicator again — the reference's
//! `onTravelingChanged`/`useLocalIndicator` handshake, which is what keeps the
//! resting pill correct on a cold start (before the springs have ever run).
//! A width transition is *not* a selection change: while the rail is
//! expanding or collapsing the pill **jumps** to the selected item's new
//! geometry every frame instead of morphing (the reference's `layoutToken` +
//! `layoutSettleDuration` remeasure loop, which this port gets for free by
//! reading the freshly laid-out rects).
//!
//! [`crate::navbar`] establishes the sibling per-item indicator for the bottom
//! bar; this module transcribes the rail's own shared-overlay indicator rather
//! than reusing it — they are different mechanisms upstream.
//!
//! # Modal presentation: painted in-widget, not through the modal host
//!
//! With [`NavigationRailModality::Modal`], the expanded rail overlays content
//! behind a scrim ([`crate::overlay::OVERLAY_SCRIM_ALPHA`], the same M3 32%),
//! and a press on the scrim reports [`NavigationRailView::on_dismiss_modal`].
//!
//! **This deliberately does not go through [`crate::overlay::modal`]**, the
//! catalog's modal host. That host is *route-like*: it pushes a transparent
//! navigator page, owns the panel's lifetime, pops with a value, and stages an
//! Android back press as its own dismiss ramp. The reference's modal rail is
//! none of those things — it inserts a bare `OverlayEntry` into the root
//! `Overlay` (`_insertOverlay`), keeps the rail widget itself mounted in
//! layout, and its lifetime is entirely a function of the `modality`/`type`
//! props the app already owns. Routing it through the modal host would give
//! the panel a second, competing owner (the navigator stack) for state the app
//! is already controlling, so the scrim is painted here instead, exactly as
//! upstream paints it.
//!
//! **What upstream actually wires for dismissal**: a scrim tap, and nothing
//! else. `_buildModalOverlay` wraps the scrim in a
//! `GestureDetector(onTap: widget.onDismissModal)`; there is no `Focus`,
//! `Shortcuts`, `KeyboardListener` or `PopScope` anywhere in the rail tree, so
//! neither **Escape** nor an Android **back** press dismisses it — despite
//! `M3ENavigationRailModality.modal`'s own doc comment claiming "dismisses on
//! tap/esc". This port matches the code, not the comment: the scrim press is
//! the only dismissal, fired on release-inside like every other press in this
//! catalog. An app that wants Escape/back handling wires it at the mount
//! (a [`frust::BackHandler`], its own key handling) and flips the prop.
//!
//! # Mounting
//!
//! A [`NavigationRailModality::Standard`] rail is an ordinary in-layout widget:
//! it self-sizes to the animated rail width and fills the height it is given,
//! so it goes directly in a [`frust::Row`] beside the page body.
//!
//! A [`NavigationRailModality::Modal`] rail is a **top layer**: it fills the
//! whole area it is given, pins the panel to the leading edge, paints the scrim
//! over the rest, and swallows presses that land on the scrim — the same
//! full-area mounting contract [`crate::overlay`]'s hosts document (the top
//! child of a full-area [`frust::Stack`], or a transparent navigator page).
//! Upstream achieves this by rendering a `SizedBox.shrink()` in place and
//! duplicating the rail into the root overlay; one widget cannot be in two
//! places here, so the port keeps a single widget whose *own* layout behaviour
//! switches with the modality. The one behavioural consequence is that a
//! collapsed **modal** rail floats over the content instead of occupying a
//! layout column the way upstream's collapsed modal rail does — while
//! collapsed it lets every press outside its panel fall through to the layer
//! below, so nothing behind it goes inert.
//!
//! Either way the rail **fills the height it is given**, so a bounded height is
//! part of the mounting contract: mounted where the vertical constraint is
//! unbounded (inside a [`frust::scroll_view`], or a column that hands its
//! children infinity), it reads a zero-height area and collapses — the same
//! trap [`crate::overlay`]'s `finite_or_zero` documents for its own hosts.
//!
//! # Semantics
//!
//! One [`Role::TabList`] container node with a [`Role::Tab`] node per
//! destination, carrying the destination's label (its `semantic_label` when
//! given) and [`frust::authoring::Node::set_selected`] — the same vocabulary
//! [`crate::navbar`] uses, and for the same reason. The menu button, the FAB
//! and the trailing slot contribute their own nodes beneath the same
//! container.
//!
//! # Not ported
//!
//! * **A scrollable destination column** (`scrollable: true`, upstream's
//!   default `ListView`) — the column lays out top-down and a rail taller than
//!   its box overflows. An app with more destinations than fit puts the rail
//!   inside its own [`frust::scroll_view`].
//! * **The collapsed "peek" overlay** (`_buildCollapsedPeekOverlay`) — when
//!   `hideWhenCollapsed` shrinks the rail to zero width, upstream floats a
//!   detached expand button into the root overlay via a
//!   `CompositedTransformFollower`. A zero-width widget here receives no
//!   pointer events at all, and this port has no seam for hosting an
//!   interactive affordance outside its own box, so
//!   [`NavigationRailView::hide_when_collapsed`] gives a genuinely empty rail
//!   and the app owns its own expand affordance.
//! * **Per-item tooltips** (collapsed items are `Tooltip`-wrapped upstream) —
//!   the destination's label already reaches assistive technology through the
//!   `Role::Tab` node, and re-wrapping a caller-supplied icon view into
//!   [`mod@crate::tooltip`] would need to *own* that view, which a `&self`
//!   `View::build` cannot do.
//! * **The icon scale-pop** (`M3ENavIconScale`, a 0.98→1.0 spring on the newly
//!   selected icon) — a per-item transform around a caller-supplied icon view,
//!   deferred with the tooltips above.
//! * **`M3EScrimSystemUi.wrap`** — Android status/navigation-bar icon
//!   styling while the modal scrim is up. That is a `frust::system_ui`
//!   concern an app drives itself, not a widget one.
//! * **`M3ENavigationRailDestination.short`** and the theme's `headerMinSpace`
//!   — both are declared upstream and read by nothing. So is
//!   `_buildChildren`'s `showLabels` argument (computed from a `maxWidth >=
//!   180` test and then never consulted), so a collapsed label's visibility
//!   here is [`RailLabelBehavior`] alone, exactly as the reference behaves.
//! * **`suppressInk`** — upstream suppresses splash/hover/highlight on both
//!   item shapes (`NoSplash.splashFactory`, transparent hover/highlight/overlay
//!   colors, `suppressInk: true` on the collapsed icon button) and then carries
//!   a flag to suppress it *harder* during a transition. A rail item here
//!   paints no state layer at all, so there is nothing left to suppress; the
//!   only press feedback is the selection indicator.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{
    FontWeight, LineHeight, TextContext, TextLayout, TextOverflow, TextStyle,
};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx,
    View, Widget, any, build_child, erase_callback, rebuild_child, route_event, route_event_single,
    teardown_child, visit_children,
};
use frust::{
    AnimationController, Curve, FrameTime, IconSource, Spring, SpringDesc, Theme, Tween, icon,
};
use kurbo::{Point, Rect, Size};
use peniko::{Brush, Color};

use crate::fab::{FabColor, FabSize, extended_fab, fab};
use crate::icon_button::icon_button;
use crate::overlay::{OVERLAY_SCRIM_ALPHA, finite_or_zero};
use crate::press::presses;

// ---- Layout tokens ---------------------------------------------------------

/// The collapsed rail's width, in logical px
/// (`M3ENavigationRailTheme.collapsedWidth`).
pub const RAIL_COLLAPSED_WIDTH: f64 = 96.0;
/// The expanded rail's minimum (and default) width, in logical px
/// (`M3ENavigationRailTheme.expandedMinWidth`).
pub const RAIL_EXPANDED_MIN_WIDTH: f64 = 220.0;
/// The expanded rail's maximum width, in logical px
/// (`M3ENavigationRailTheme.expandedMaxWidth`).
pub const RAIL_EXPANDED_MAX_WIDTH: f64 = 360.0;
/// A collapsed destination's total height, in logical px
/// (`M3ENavigationRailTheme.itemCollapsedHeight`).
pub const RAIL_ITEM_COLLAPSED_HEIGHT: f64 = 66.0;
/// An expanded destination row's *minimum* height, in logical px
/// (`M3ENavigationRailTheme.itemExpandedHeight`) — a row with a taller
/// measured label grows past it.
pub const RAIL_ITEM_EXPANDED_HEIGHT: f64 = 40.0;

/// The destination icon's side length, in logical px
/// (`M3ENavigationRailTheme.iconSize`).
const ICON_SIZE: f64 = 24.0;
/// Leading inset inside an expanded row, in logical px
/// (`M3ENavigationRailTheme.indicatorLeading`).
const INDICATOR_LEADING: f64 = 16.0;
/// Trailing inset inside an expanded row, in logical px
/// (`M3ENavigationRailTheme.indicatorTrailing`).
const INDICATOR_TRAILING: f64 = 16.0;
/// Gap between a row's icon and its label, in logical px
/// (`M3ENavigationRailTheme.iconLabelGap`).
const ICON_LABEL_GAP: f64 = 8.0;
/// Vertical padding above *and* below every destination, in logical px
/// (`M3ENavigationRailTheme.itemVerticalGap`).
const ITEM_VERTICAL_GAP: f64 = 4.0;
/// Space above a section header, in logical px
/// (`M3ENavigationRailTheme.sectionHeaderSpacingTop`).
const SECTION_HEADER_SPACING_TOP: f64 = 12.0;
/// Space below a section header, in logical px
/// (`M3ENavigationRailTheme.sectionHeaderSpacingBottom`).
const SECTION_HEADER_SPACING_BOTTOM: f64 = 8.0;
/// Horizontal inset applied to every row of rail content, in logical px
/// (`M3ENavigationRailLayout.horizontalInset`, and the `start`/`end` of its
/// `sectionPadding`).
const HORIZONTAL_INSET: f64 = 16.0;
/// Bottom inset under the menu button / FAB / trailing slot, in logical px
/// (`M3ENavigationRailLayout.sectionPadding`'s `bottom`).
const SECTION_PADDING_BOTTOM: f64 = 12.0;
/// Blank space above the menu button, in logical px
/// (`M3ENavigationRailLayout.topGap`).
const TOP_GAP: f64 = 36.0;

/// A collapsed destination's icon-chip width, in logical px — the *visual*
/// box [`crate::icon_button`] paints for
/// [`IconButtonSize::Sm`](crate::icon_button::IconButtonSize::Sm) ×
/// [`IconButtonWidth::Wide`](crate::icon_button::IconButtonWidth::Wide), which
/// is the pair the reference's collapsed item builds
/// (`M3EIconButton(width: M3EIconButtonWidth.wide)`).
const CHIP_WIDTH: f64 = 52.0;
/// That chip's visual height, in logical px (same source).
const CHIP_HEIGHT: f64 = 40.0;
/// That chip's tap-target height, in logical px — the 48dp minimum the icon
/// button pads its `Sm` visual box out to.
const CHIP_TARGET_HEIGHT: f64 = 48.0;

/// The badge dot's diameter, in logical px (`M3ERailBadge`'s `count == 0`
/// branch: a bare 8×8 circle).
const BADGE_DOT: f64 = 8.0;
/// Horizontal padding inside a labeled badge pill, in logical px
/// (`M3ERailBadge`'s `pad + 2` with the non-dense `pad = 4`).
const BADGE_H_PAD: f64 = 6.0;
/// Vertical padding inside a labeled badge pill, in logical px
/// (`M3ERailBadge`'s non-dense `pad`).
const BADGE_V_PAD: f64 = 4.0;
/// Highest badge count rendered verbatim; anything above shows `999+`
/// (`M3ERailBadge.maxDigits`'s documented 3-digit cap).
///
/// The reference's own threshold expression is `count > 10 * (10^maxDigits -
/// 1)` — 9990 for the default `maxDigits: 3` — which contradicts both its
/// doc comment ("Maximum digits before showing a trailing '+', e.g. 999+")
/// and the `999+` string it then renders, leaving 1000..=9990 as
/// four-digit labels in a pill sized for three. This port takes the
/// documented contract.
const BADGE_MAX_COUNT: u32 = 999;

// ---- Motion tokens ---------------------------------------------------------

/// The width transition's duration (`M3ENavigationRailLayout.expandDuration`).
pub const RAIL_EXPAND_DURATION: Duration = Duration::from_millis(280);

/// The width transition's easing — Flutter's `Curves.easeOutCubic`, whose
/// control points are `(0.215, 0.61)`/`(0.355, 1.0)`. Not
/// [`frust::Curve::EaseOut`], which is CSS `ease-out`
/// (`cubic-bezier(0, 0, 0.58, 1)`) and a visibly different ramp.
pub const RAIL_EXPAND_CURVE: Curve = Curve::Cubic(0.215, 0.61, 0.355, 1.0);

/// The liquid indicator's **lead**-edge spring: the reference's
/// `expressiveSpatialDefault` stiffness with `damping: 0.45`
/// (`_leadMotion`). Restated as a [`SpringDesc`] rather than read from a
/// theme for the reason [`mod@crate::expandable_list`]'s `REVEAL_SPRING`
/// documents — a `fling` needs one, and paint/event code reads no theme for
/// motion. A test pins the stiffness to
/// [`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`](crate::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT).
pub const RAIL_LEAD_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.45,
};

/// The liquid indicator's **trailing**-edge spring: the same stiffness with
/// `damping: 0.55` (`_trailMotion`) — more damped than
/// [`RAIL_LEAD_SPRING`], which is what makes the pill stretch on the way.
pub const RAIL_TRAIL_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};

/// Rest threshold for a travelling edge, in logical px (and px/s for its
/// velocity) — both are checked, per `Spring::is_at_rest`'s own contract —
/// the same value `crate::navbar`'s module-private `PILL_REST_EPSILON` uses
/// for the sibling liquid indicators.
const TRAVEL_REST_EPSILON: f64 = 0.01;

/// Distance in logical px under which two indicator positions count as the
/// same place (a retarget guard, so an unchanged layout never relaunches a
/// settled spring).
const TRAVEL_EPSILON: f64 = 0.5;

// ---- Type tokens -----------------------------------------------------------

/// One type-scale row as `(size, line_height, letter_spacing, weight)` — the
/// unthemed fallback for a text role, mirroring
/// [`crate::button`]'s own `TypeToken` shape.
type TypeToken = (f32, f32, f32, FontWeight);

/// `label_large` — an expanded destination's label
/// (`m3e_rail_item_button.dart`'s `typeScale.labelLarge`).
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
/// `label_medium` — a collapsed destination's label (`typeScale.labelMedium`).
const LABEL_MEDIUM: TypeToken = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
/// `label_small` — a badge's count (`M3ERailBadge`'s `typeScale.labelSmall`).
const LABEL_SMALL: TypeToken = (11.0, 16.0, 0.5, FontWeight::MEDIUM);
/// `title_small` — a section header (`_sectionHeader`'s `typeScale.titleSmall`).
const TITLE_SMALL: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);

/// The ink every run is shaped with; paint re-brushes it to the resolved role
/// color (the [`crate::button`]/[`crate::icon_button`] run convention).
const SHAPING_INK: Color = Color::BLACK;

// ---- Color fallbacks -------------------------------------------------------

/// Unthemed-fallback `surface` — the rail container
/// (`containerColorResolved`).
const SURFACE: Color = Color::from_rgb8(0xFE, 0xF7, 0xFF);
/// Unthemed-fallback `secondary_container` — the selection indicator
/// (`activeIndicatorColorResolved`).
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `on_secondary_container` — a selected destination's icon
/// and label (`activeIconAndLabelColor`).
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `on_surface_variant` — an unselected destination's label
/// and every section header (`inactiveIconAndLabelColor`).
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `primary` — a badge's fill (`badgeBackground`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `on_primary` — a badge's label (`badgeLargeLabel`).
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback `scrim`, painted at [`OVERLAY_SCRIM_ALPHA`].
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Return `color` with its alpha channel replaced by `alpha` (the same helper
/// [`crate::navbar`] and [`crate::overlay`] each carry).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (the same helper [`crate::navbar`] carries).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Resolve `token` (or the theme's matching role) into a shapeable style,
/// always carrying [`SHAPING_INK`].
fn type_style(themed: Option<&TextStyle>, token: TypeToken) -> TextStyle {
    let mut style = match themed {
        Some(style) => style.clone(),
        None => {
            let (size, line_height, letter_spacing, weight) = token;
            let mut style = TextStyle::new(size, SHAPING_INK);
            style.line_height = LineHeight::Absolute(line_height);
            style.letter_spacing = letter_spacing;
            style.weight = weight;
            style
        }
    };
    style.color = SHAPING_INK;
    style
}

// ---- Enums -----------------------------------------------------------------

/// Which rail shape the caller is asking for — the reference's
/// `M3ENavigationRailType` (`enums/m3e_navigation_rail_enums.dart`).
///
/// The two `Always*` variants differ from their plain counterparts only in
/// dropping the menu button: nothing can toggle them from inside the rail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavigationRailType {
    /// The slim icon rail, with a menu button that requests expansion.
    Collapsed,
    /// The slim icon rail with no menu button at all.
    AlwaysCollapse,
    /// The wide icon-plus-label rail, with a menu button that requests
    /// collapse (the reference's own default).
    #[default]
    Expanded,
    /// The wide rail with no menu button at all.
    AlwaysExpand,
}

impl NavigationRailType {
    /// Whether this type shows labels beside icons and lays destinations out as
    /// full-width rows.
    pub fn is_expanded(self) -> bool {
        matches!(self, Self::Expanded | Self::AlwaysExpand)
    }

    /// Whether the rail offers a menu button at all (the reference's
    /// `_canToggle`: the plain `collapsed`/`expanded` pair only).
    pub fn can_toggle(self) -> bool {
        matches!(self, Self::Collapsed | Self::Expanded)
    }

    /// The type the menu button requests next — [`Self::Expanded`] from
    /// collapsed, [`Self::Collapsed`] from expanded.
    fn toggled(self) -> Self {
        if self.is_expanded() {
            Self::Collapsed
        } else {
            Self::Expanded
        }
    }
}

/// How the expanded rail relates to the content beside it — the reference's
/// `M3ENavigationRailModality`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavigationRailModality {
    /// The rail occupies layout space beside the content (the default).
    #[default]
    Standard,
    /// The rail floats over the content behind a scrim; a scrim press reports
    /// [`NavigationRailView::on_dismiss_modal`]. See the [module docs](self)'
    /// modal section for the mounting contract.
    Modal,
}

/// When a **collapsed** destination shows its text label — the reference's
/// `M3ENavigationRailLabelBehavior`.
///
/// An expanded row always shows its label; the reference consults this enum
/// only on the collapsed shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RailLabelBehavior {
    /// Every destination shows its label (the default).
    #[default]
    AlwaysShow,
    /// Only the selected destination shows its label.
    OnlySelected,
    /// No destination shows a label.
    AlwaysHide,
}

impl RailLabelBehavior {
    /// Whether a collapsed destination shows its label
    /// (`_buildCollapsedContent`'s `showLabel`).
    fn shows(self, selected: bool) -> bool {
        match self {
            Self::AlwaysShow => true,
            Self::OnlySelected => selected,
            Self::AlwaysHide => false,
        }
    }
}

// ---- Models ----------------------------------------------------------------

/// One rail destination — the reference's `M3ENavigationRailDestination`.
pub struct RailDestination<State: 'static> {
    icon: AnyView<State>,
    selected_icon: Option<AnyView<State>>,
    label: String,
    badge_count: Option<u32>,
    semantic_label: Option<String>,
}

/// Create a destination showing `icon` above/beside `label`.
///
/// Tint is the supplied icon view's own responsibility, the same stance
/// [`crate::navbar`]'s and [`crate::appbar`]'s icon slots take.
pub fn rail_destination<State: 'static>(
    icon: AnyView<State>,
    label: impl Into<String>,
) -> RailDestination<State> {
    RailDestination {
        icon,
        selected_icon: None,
        label: label.into(),
        badge_count: None,
        semantic_label: None,
    }
}

impl<State: 'static> RailDestination<State> {
    /// A distinct icon to show while this destination is selected (falls back
    /// to the base icon).
    pub fn selected_icon(mut self, icon: AnyView<State>) -> Self {
        self.selected_icon = Some(icon);
        self
    }

    /// A numeric badge; `0` paints the reference's bare 8dp dot.
    pub fn badge_count(mut self, count: u32) -> Self {
        self.badge_count = Some(count);
        self
    }

    /// The accessible name, when the visible label isn't the right one.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// The icon view showing right now (`selected_icon` while selected, if
    /// one was given) — the same resolution [`crate::icon_button`]'s
    /// `active_icon` performs.
    fn active_icon(&self, selected: bool) -> &AnyView<State> {
        if selected {
            self.selected_icon.as_ref().unwrap_or(&self.icon)
        } else {
            &self.icon
        }
    }
}

/// A labeled group of destinations — the reference's
/// `M3ENavigationRailSection`.
///
/// The header shows only on the **expanded** rail: upstream's collapsed pass
/// (`_buildCollapsedDestinations`) flattens every section's destinations and
/// drops the headers entirely.
pub struct RailSection<State: 'static> {
    header: Option<String>,
    destinations: Vec<RailDestination<State>>,
}

/// Create an unlabeled section over `destinations` (chain
/// [`RailSection::header`] to label it).
pub fn rail_section<State: 'static>(
    destinations: Vec<RailDestination<State>>,
) -> RailSection<State> {
    RailSection {
        header: None,
        destinations,
    }
}

impl<State: 'static> RailSection<State> {
    /// Label this section (`titleSmall`, `onSurfaceVariant`), as a single
    /// ellipsized line — the same one-line treatment every other label in this
    /// rail takes.
    ///
    /// Upstream takes an arbitrary header `Widget` and then imposes that exact
    /// text style on it with a `DefaultTextStyle`; this port narrows the slot
    /// to the string that styling is for.
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }
}

/// The rail's built-in FAB slot — the reference's
/// `M3ENavigationRailFabSlot`.
///
/// Renders as a plain [`mod@crate::fab`] while collapsed and an
/// [`crate::extended_fab`] while expanded, exactly as upstream switches
/// between `M3EFab` and `M3EExtendedFab`. The icon is an [`IconSource`]
/// rather than an arbitrary view because this slot is *re-wrapped* into a
/// catalog FAB view on every rebuild, which a non-clonable
/// [`AnyView`] cannot be (destination and trailing slots, which become child
/// pods directly, still take views).
pub struct RailFab<State: 'static> {
    icon: IconSource,
    label: String,
    on_press: Rc<dyn Fn(&mut State)>,
    color: FabColor,
    size: FabSize,
    semantic_label: Option<String>,
}

/// Create the rail's FAB slot: `icon` alone while collapsed, `icon` + `label`
/// while expanded, firing `on_press` on release inside.
pub fn rail_fab<State: 'static, F: Fn(&mut State) + 'static>(
    icon: IconSource,
    label: impl Into<String>,
    on_press: F,
) -> RailFab<State> {
    RailFab {
        icon,
        label: label.into(),
        on_press: Rc::new(on_press),
        color: FabColor::Primary,
        size: FabSize::Medium,
        semantic_label: None,
    }
}

impl<State: 'static> RailFab<State> {
    /// The container color tier (default [`FabColor::Primary`]).
    pub fn color(mut self, color: FabColor) -> Self {
        self.color = color;
        self
    }

    /// The collapsed FAB's size tier (default [`FabSize::Medium`]); the
    /// expanded variant's height is fixed by [`crate::extended_fab`].
    pub fn size(mut self, size: FabSize) -> Self {
        self.size = size;
        self
    }

    /// The accessible name for the collapsed (icon-only) FAB; defaults to the
    /// slot's own label.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// The presence/identity shape a rebuild diffs on before re-wrapping.
    fn shape(&self) -> (&str, FabColor, FabSize, Option<&str>) {
        (
            self.label.as_str(),
            self.color,
            self.size,
            self.semantic_label.as_deref(),
        )
    }
}

// ---- Text runs -------------------------------------------------------------

/// A lazily shaped, paint-time-rebrushed text run — the same idiom
/// [`crate::icon_button`]'s `BadgeRun` and `crate::button`'s `LabelRun` use,
/// scoped to what a rail label / section header / badge count needs.
struct TextRun {
    content: String,
    layout: Option<TextLayout>,
    shaped_for: Option<(TextStyle, Option<f64>)>,
}

impl TextRun {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_for: None,
        }
    }

    /// Replace the run's text, invalidating any cached shaping. Returns
    /// whether anything changed.
    fn set_content(&mut self, content: &str) -> bool {
        if self.content == content {
            return false;
        }
        self.content = content.to_string();
        self.layout = None;
        self.shaped_for = None;
        true
    }

    /// Shape (or reuse) the run, fitted to `max_width` as a **single
    /// ellipsized line** when one is given — the reference's every label
    /// carries `maxLines: 1, overflow: TextOverflow.ellipsis`.
    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f64>) -> Size {
        let key = (style.clone(), max_width);
        if let Some(layout) = &self.layout
            && self.shaped_for.as_ref() == Some(&key)
        {
            return layout.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = match max_width {
            Some(width) => text_ctx.layout_bounded(
                &self.content,
                style,
                Some(width as f32),
                Some(1),
                TextOverflow::Ellipsis,
            ),
            None => text_ctx.layout(&self.content, style, None),
        };
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(key);
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding the [`SHAPING_INK`] it
    /// was shaped with. A never-shaped run paints nothing.
    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ---- Motion drivers --------------------------------------------------------

/// The rail's animated width: a duration+curve ramp between two widths (see
/// the [module docs](self)' width-transition section).
struct WidthMotion {
    /// The width `layout` reads this frame.
    value: f64,
    /// The width the current ramp is heading for.
    target: f64,
    /// Maps the controller's `0 → 1` ramp onto `from → target`.
    tween: Tween<f64>,
    driver: AnimationController,
}

impl WidthMotion {
    /// A motion resting at `width` with no ramp in flight — a rail built
    /// already expanded shows its full width on the first frame rather than
    /// animating into it.
    fn resting(width: f64) -> Self {
        Self {
            value: width,
            target: width,
            tween: Tween::new(width, width),
            driver: AnimationController::new(RAIL_EXPAND_DURATION).with_curve(RAIL_EXPAND_CURVE),
        }
    }

    /// Ramp toward `target` (a no-op if already heading there).
    fn set_target(&mut self, target: f64) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.tween = Tween::new(self.value, target);
        self.driver = AnimationController::new(RAIL_EXPAND_DURATION).with_curve(RAIL_EXPAND_CURVE);
        self.driver.forward();
    }

    /// Jump to the target, cancelling any ramp (the reduce-motion path).
    /// Returns whether that actually moved the width.
    fn snap(&mut self) -> bool {
        let moved = self.value != self.target;
        self.value = self.target;
        self.tween = Tween::new(self.target, self.target);
        self.driver.stop();
        moved
    }

    /// Advance one frame, returning whether the ramp is still in flight.
    fn advance(&mut self, now: FrameTime) -> bool {
        let animating = self.driver.advance(now);
        self.value = self.tween.lerp(self.driver.value_clamped());
        animating
    }

    fn is_animating(&self) -> bool {
        self.driver.is_animating()
    }
}

/// One edge of the liquid selection indicator: a spring-driven main-axis
/// center.
///
/// Built directly on the analytic [`Spring`] rather than a duration-seeded
/// [`AnimationController`] fling — the same shape as [`crate::navbar`]'s
/// `LiquidEdge`, which this type transcribes (see that module's own doc
/// comment for why `navbar.rs` itself stays read-only here). A [`Spring`]
/// solves *displacement from equilibrium*, so the live position is `target +
/// spring.position(elapsed)`; [`Self::travel_to`] re-solves from the current
/// position **and velocity**, which is what keeps a fast double-tap between
/// destinations reversing smoothly instead of relaunching from a near-zero
/// rate.
struct TravelAxis {
    /// The edge center this frame, in rail-local px.
    value: f64,
    /// The center the current leg is heading for.
    target: f64,
    /// The in-flight analytic solution, or `None` once settled/jumped.
    flight: Option<Spring>,
    /// Seconds since [`Self::flight`] was solved.
    elapsed: f64,
    /// The last-solved instantaneous velocity, px/s — `0.0` once settled,
    /// carried into the next [`Self::travel_to`]'s initial condition.
    velocity: f64,
    /// Clock for `advance`'s delta; `None` re-seeds it on the next call.
    last_time: Option<FrameTime>,
}

impl TravelAxis {
    fn new() -> Self {
        Self {
            value: 0.0,
            target: 0.0,
            flight: None,
            elapsed: 0.0,
            velocity: 0.0,
            last_time: None,
        }
    }

    /// Snap to `to`, cancelling any leg (a first measure, a layout change, or
    /// reduce motion).
    fn jump(&mut self, to: f64) {
        self.value = to;
        self.target = to;
        self.flight = None;
        self.elapsed = 0.0;
        self.velocity = 0.0;
        self.last_time = None;
    }

    /// Launch a fresh leg toward `to` on `spring` (a no-op if already heading
    /// there), re-solving from wherever the edge currently reads **and its
    /// current velocity** so an interrupted travel reverses smoothly — the
    /// [`crate::navbar`] `LiquidEdge::retarget` carry, transcribed.
    fn travel_to(&mut self, to: f64, spring: SpringDesc) {
        if (self.target - to).abs() < TRAVEL_EPSILON {
            return;
        }
        self.flight = Some(Spring::new(spring, self.value - to, self.velocity));
        self.target = to;
        self.elapsed = 0.0;
        self.last_time = None;
    }

    /// Advance to frame time `now`, returning whether the leg is still in
    /// flight.
    ///
    /// The position is read **unclamped**: the lead spring is under-damped on
    /// purpose and its overshoot past the target is the stretch that makes
    /// the pill read as liquid.
    fn advance(&mut self, now: FrameTime) -> bool {
        let Some(spring) = self.flight else {
            return false;
        };
        let dt = match self.last_time {
            Some(last) => now.saturating_sub(last).as_secs_f64(),
            None => 0.0,
        };
        self.last_time = Some(now);
        self.elapsed += dt;
        if spring.is_at_rest(self.elapsed, TRAVEL_REST_EPSILON) {
            self.value = self.target;
            self.velocity = 0.0;
            self.flight = None;
            self.elapsed = 0.0;
            self.last_time = None;
            return false;
        }
        self.value = self.target + spring.position(self.elapsed);
        self.velocity = spring.velocity(self.elapsed);
        true
    }

    fn is_animating(&self) -> bool {
        self.flight.is_some()
    }
}

// ---- The view --------------------------------------------------------------

/// A view-held, typed selection callback (erased per-destination on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;
/// A view-held, typed expansion-request callback.
type OnTypeChanged<State> = Rc<dyn Fn(&mut State, NavigationRailType)>;
/// A view-held, typed scrim-dismiss callback.
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 Expressive navigation rail. See the [module docs](self).
pub struct NavigationRailView<State: 'static> {
    sections: Vec<RailSection<State>>,
    selected: usize,
    on_select: OnSelect<State>,
    rail_type: NavigationRailType,
    modality: NavigationRailModality,
    fab: Option<RailFab<State>>,
    hide_when_collapsed: bool,
    expanded_width: Option<f64>,
    on_dismiss_modal: Option<OnDismiss<State>>,
    on_type_changed: Option<OnTypeChanged<State>>,
    label_behavior: RailLabelBehavior,
    trailing: Option<AnyView<State>>,
    trailing_at_bottom: bool,
    background: Option<Color>,
}

/// Create a navigation rail over `sections`, with `selected` the current
/// (app-confirmed) **flat** destination index — counted across every section
/// in order, exactly as upstream's `onDestinationSelected` reports it.
///
/// Fires `on_select(state, index)` on a release inside a destination. A
/// **controlled** component: `selected` is never mutated here; the app feeds
/// the confirmed index back in on the next rebuild.
pub fn navigation_rail<State: 'static, F: Fn(&mut State, usize) + 'static>(
    sections: Vec<RailSection<State>>,
    selected: usize,
    on_select: F,
) -> NavigationRailView<State> {
    NavigationRailView {
        sections,
        selected,
        on_select: Rc::new(on_select),
        rail_type: NavigationRailType::default(),
        modality: NavigationRailModality::default(),
        fab: None,
        hide_when_collapsed: false,
        expanded_width: None,
        on_dismiss_modal: None,
        on_type_changed: None,
        label_behavior: RailLabelBehavior::default(),
        trailing: None,
        trailing_at_bottom: true,
        background: None,
    }
}

/// PascalCase alias for [`navigation_rail`], matching the catalog's container
/// view-fn vocabulary.
#[allow(non_snake_case)]
pub fn NavigationRail<State: 'static, F: Fn(&mut State, usize) + 'static>(
    sections: Vec<RailSection<State>>,
    selected: usize,
    on_select: F,
) -> NavigationRailView<State> {
    navigation_rail(sections, selected, on_select)
}

impl<State: 'static> NavigationRailView<State> {
    /// The rail shape (default [`NavigationRailType::Expanded`], the
    /// reference's own default).
    pub fn rail_type(mut self, rail_type: NavigationRailType) -> Self {
        self.rail_type = rail_type;
        self
    }

    /// Standard (in-layout) or modal (over a scrim) presentation — see the
    /// [module docs](self)' modal section for the mounting contract.
    pub fn modality(mut self, modality: NavigationRailModality) -> Self {
        self.modality = modality;
        self
    }

    /// Attach the rail's built-in FAB slot.
    pub fn fab(mut self, fab: RailFab<State>) -> Self {
        self.fab = Some(fab);
        self
    }

    /// Collapse to *zero* width instead of 96dp. The reference floats a
    /// detached expand button in the root overlay for this case; this port
    /// does not (see the [module docs](self)' Not ported), so the app owns its
    /// own expand affordance.
    pub fn hide_when_collapsed(mut self, hide: bool) -> Self {
        self.hide_when_collapsed = hide;
        self
    }

    /// The expanded width, clamped to
    /// [`RAIL_EXPANDED_MIN_WIDTH`]..=[`RAIL_EXPANDED_MAX_WIDTH`] (default: the
    /// minimum).
    pub fn expanded_width(mut self, width: f64) -> Self {
        self.expanded_width = Some(width);
        self
    }

    /// Called when the modal scrim is pressed. The rail does **not** collapse
    /// itself — it reports the request and the app flips
    /// [`Self::rail_type`]/[`Self::modality`].
    pub fn on_dismiss_modal<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss_modal = Some(Rc::new(on_dismiss));
        self
    }

    /// Called with the *requested* next type when the menu button is pressed.
    /// Without it the menu button still renders and still presses, but nothing
    /// observes the request — the same shape a [`mod@crate::checkbox`] with no
    /// `on_toggle` takes.
    pub fn on_type_changed<F: Fn(&mut State, NavigationRailType) + 'static>(
        mut self,
        on_changed: F,
    ) -> Self {
        self.on_type_changed = Some(Rc::new(on_changed));
        self
    }

    /// When a **collapsed** destination shows its label (default
    /// [`RailLabelBehavior::AlwaysShow`]).
    pub fn label_behavior(mut self, behavior: RailLabelBehavior) -> Self {
        self.label_behavior = behavior;
        self
    }

    /// A trailing slot below the destinations (a settings button, an avatar).
    pub fn trailing(mut self, trailing: AnyView<State>) -> Self {
        self.trailing = Some(trailing);
        self
    }

    /// Whether the trailing slot pins to the rail's bottom edge (default
    /// `true`, matching `trailingAtBottom`) or simply follows the last
    /// destination.
    pub fn trailing_at_bottom(mut self, at_bottom: bool) -> Self {
        self.trailing_at_bottom = at_bottom;
        self
    }

    /// Override the container fill (default: the theme's `surface`).
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Every destination, flattened across sections in order — the index space
    /// `selected`/`on_select` speak.
    fn destinations(&self) -> Vec<&RailDestination<State>> {
        self.sections
            .iter()
            .flat_map(|section| section.destinations.iter())
            .collect()
    }

    /// The rail's resting width for the current props.
    fn target_width(&self) -> f64 {
        if self.rail_type.is_expanded() {
            self.expanded_width
                .unwrap_or(RAIL_EXPANDED_MIN_WIDTH)
                .clamp(RAIL_EXPANDED_MIN_WIDTH, RAIL_EXPANDED_MAX_WIDTH)
        } else if self.hide_when_collapsed {
            0.0
        } else {
            RAIL_COLLAPSED_WIDTH
        }
    }

    /// The menu button's view for the current type, or `None` when this type
    /// carries no toggle (`_buildMenuButton`'s `_canToggle` guard).
    fn menu_view(&self) -> Option<AnyView<State>> {
        if !self.rail_type.can_toggle() {
            return None;
        }
        let expanded = self.rail_type.is_expanded();
        let next = self.rail_type.toggled();
        let callback = self.on_type_changed.clone();
        // The reference's own tooltip strings, carried as accessible names —
        // this port ships no per-item tooltips (see Not ported).
        let (source, label) = if expanded {
            (crate::icons::MENU_OPEN, "Collapse")
        } else {
            (crate::icons::MENU, "Expand")
        };
        Some(any(icon_button(
            any(icon(source)),
            move |state: &mut State| {
                if let Some(callback) = &callback {
                    callback(state, next);
                }
            },
        )
        .semantic_label(label)))
    }

    /// The FAB slot's view for the current type: extended while expanded, a
    /// plain FAB while collapsed (`_buildFab`).
    fn fab_view(&self) -> Option<AnyView<State>> {
        let slot = self.fab.as_ref()?;
        let on_press = slot.on_press.clone();
        if self.rail_type.is_expanded() {
            Some(any(extended_fab(
                slot.label.clone(),
                move |state: &mut State| {
                    on_press(state);
                },
            )
            .icon(any(icon(slot.icon)))
            .color(slot.color)))
        } else {
            let label = slot
                .semantic_label
                .clone()
                .unwrap_or_else(|| slot.label.clone());
            Some(any(fab(any(icon(slot.icon)), move |state: &mut State| {
                on_press(state);
            })
            .color(slot.color)
            .size(slot.size)
            .label(label)))
        }
    }
}

/// Erase `on_select` into a per-destination callback that always reports
/// `index` (the same closed-over-index adapter [`crate::navbar`] carries).
fn item_on_select<State: 'static>(on_select: &OnSelect<State>, index: usize) -> ErasedCallback {
    let callback = on_select.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state, index);
    })
}

// ---- The destination item --------------------------------------------------

/// One destination's retained content — a plain [`Widget`] (not
/// `View`/[`AnyView`]-erased, since there is only ever one concrete item type)
/// wrapped in a [`ChildPod`] by [`NavigationRailWidget`], the same shape
/// [`crate::navbar`]'s items take.
struct RailItemWidget {
    /// The active icon view's pod (base or selected icon).
    icon: ChildPod,
    /// The visible label, also the accessible name when no
    /// `semantic_label` was given.
    label: TextRun,
    semantic_label: Option<String>,
    /// The badge count, if any (`0` paints a bare dot).
    badge_count: Option<u32>,
    badge: TextRun,
    selected: bool,
    /// Whether the rail is currently in its expanded shape.
    expanded: bool,
    label_behavior: RailLabelBehavior,
    /// Whether this item paints its own resting indicator — cleared by the
    /// rail while the shared liquid pill is traveling
    /// (`useLocalIndicator`).
    use_local_indicator: bool,
    /// The indicator rect in item-local space: the icon chip while collapsed,
    /// the whole row while expanded.
    indicator: Rect,
    label_origin: Point,
    badge_origin: Point,
    badge_size: Size,
    on_select: ErasedCallback,
    /// The pressed visual/gesture state — armed by a `Down`, cleared on
    /// `Up`/`Cancel` ([`frust::Button`]'s fire-on-up-inside contract).
    captured: bool,
}

impl RailItemWidget {
    /// Whether this item's label paints at all.
    fn shows_label(&self) -> bool {
        self.expanded || self.label_behavior.shows(self.selected)
    }

    /// The badge's resolved text, or `None` for a bare dot / no badge.
    fn badge_text(&self) -> Option<String> {
        match self.badge_count {
            None | Some(0) => None,
            Some(count) if count > BADGE_MAX_COUNT => Some(format!("{BADGE_MAX_COUNT}+")),
            Some(count) => Some(count.to_string()),
        }
    }
}

/// Build one destination's retained pod.
fn build_item<State: 'static>(
    destination: &RailDestination<State>,
    selected: bool,
    expanded: bool,
    label_behavior: RailLabelBehavior,
    on_select: &OnSelect<State>,
    index: usize,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let widget = RailItemWidget {
        icon: build_child(destination.active_icon(selected), ctx),
        label: TextRun::new(destination.label.clone()),
        semantic_label: destination.semantic_label.clone(),
        badge_count: destination.badge_count,
        badge: TextRun::new(String::new()),
        selected,
        expanded,
        label_behavior,
        use_local_indicator: true,
        indicator: Rect::ZERO,
        label_origin: Point::ZERO,
        badge_origin: Point::ZERO,
        badge_size: Size::ZERO,
        on_select: item_on_select::<State>(on_select, index),
        captured: false,
    };
    ChildPod::new(Box::new(widget))
}

impl Widget for RailItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = finite_or_zero(bc.max().width);
        let theme = Theme::from_layout_ctx(ctx);
        let label_style = type_style(
            theme.map(|t| {
                if self.expanded {
                    &t.type_scale.label_large
                } else {
                    &t.type_scale.label_medium
                }
            }),
            if self.expanded {
                LABEL_LARGE
            } else {
                LABEL_MEDIUM
            },
        );
        let badge_style = type_style(theme.map(|t| &t.type_scale.label_small), LABEL_SMALL);

        // The badge's own box first — an expanded row's label is fitted to
        // whatever the badge leaves behind.
        let badge_text = self.badge_text();
        self.badge_size = match (&badge_text, self.badge_count) {
            (Some(text), _) => {
                self.badge.set_content(text);
                let measured = self.badge.shape(ctx, &badge_style, None);
                Size::new(
                    measured.width + BADGE_H_PAD * 2.0,
                    measured.height + BADGE_V_PAD * 2.0,
                )
            }
            (None, Some(_)) => Size::new(BADGE_DOT, BADGE_DOT),
            (None, None) => Size::ZERO,
        };

        if self.expanded {
            let badge_slot = if self.badge_size.width > 0.0 {
                self.badge_size.width + ICON_LABEL_GAP
            } else {
                0.0
            };
            let label_max = (width
                - INDICATOR_LEADING
                - INDICATOR_TRAILING
                - ICON_SIZE
                - ICON_LABEL_GAP
                - badge_slot)
                .max(0.0);
            let label_size = self.label.shape(ctx, &label_style, Some(label_max));
            let content_height = label_size.height.max(ICON_SIZE).max(self.badge_size.height);
            let height = content_height.max(RAIL_ITEM_EXPANDED_HEIGHT);

            let icon_measured = self
                .icon
                .layout_child(ctx, &BoxConstraints::tight(Size::new(ICON_SIZE, ICON_SIZE)));
            self.icon.set_origin(Point::new(
                INDICATOR_LEADING,
                (height - icon_measured.height) / 2.0,
            ));
            self.label_origin = Point::new(
                INDICATOR_LEADING + ICON_SIZE + ICON_LABEL_GAP,
                (height - label_size.height) / 2.0,
            );
            self.badge_origin = Point::new(
                (width - INDICATOR_TRAILING - self.badge_size.width).max(0.0),
                (height - self.badge_size.height) / 2.0,
            );
            // The expanded indicator *is* the row.
            self.indicator = Rect::new(0.0, 0.0, width, height);
            return bc.constrain(Size::new(width, height));
        }

        // Collapsed: a left-aligned icon chip inside its 48dp tap target, with
        // the label (when shown) directly beneath, the pair centered in the
        // item's fixed height.
        let label_size = if self.shows_label() {
            self.label.shape(ctx, &label_style, Some(width))
        } else {
            Size::ZERO
        };
        let content_height = CHIP_TARGET_HEIGHT + label_size.height;
        let top = ((RAIL_ITEM_COLLAPSED_HEIGHT - content_height) / 2.0).max(0.0);
        let chip = Rect::new(
            0.0,
            top + (CHIP_TARGET_HEIGHT - CHIP_HEIGHT) / 2.0,
            CHIP_WIDTH.min(width),
            top + (CHIP_TARGET_HEIGHT + CHIP_HEIGHT) / 2.0,
        );
        let icon_measured = self
            .icon
            .layout_child(ctx, &BoxConstraints::tight(Size::new(ICON_SIZE, ICON_SIZE)));
        self.icon.set_origin(Point::new(
            chip.x0 + (chip.width() - icon_measured.width) / 2.0,
            chip.y0 + (chip.height() - icon_measured.height) / 2.0,
        ));
        self.label_origin = Point::new(
            chip.x0 + (chip.width() - label_size.width) / 2.0,
            top + CHIP_TARGET_HEIGHT,
        );
        // Badge at the chip's top-right corner, the placement
        // `crate::icon_button` gives its own badge slot.
        self.badge_origin = Point::new(chip.x1 - self.badge_size.width, chip.y0);
        self.indicator = chip;
        bc.constrain(Size::new(width, RAIL_ITEM_COLLAPSED_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let indicator_fill = theme.map_or(SECONDARY_CONTAINER, |t| t.scheme().secondary_container);
        let content = if self.selected {
            theme.map_or(ON_SECONDARY_CONTAINER, |t| {
                t.scheme().on_secondary_container
            })
        } else {
            theme.map_or(ON_SURFACE_VARIANT, |t| t.scheme().on_surface_variant)
        };
        let badge_bg = theme.map_or(PRIMARY, |t| t.scheme().primary);
        let badge_fg = theme.map_or(ON_PRIMARY, |t| t.scheme().on_primary);
        let origin = ctx.origin();

        if self.selected && self.use_local_indicator {
            let rect = self.indicator;
            scene.fill_rounded_rect(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                rect.size(),
                rect.width().min(rect.height()) / 2.0,
                indicator_fill,
            );
        }

        self.icon.paint_child(ctx, scene);

        if self.shows_label() {
            self.label.paint(
                Point::new(
                    origin.x + self.label_origin.x,
                    origin.y + self.label_origin.y,
                ),
                content,
                scene,
            );
        }

        if self.badge_size.width > 0.0 {
            let pill = Point::new(
                origin.x + self.badge_origin.x,
                origin.y + self.badge_origin.y,
            );
            scene.fill_rounded_rect(
                pill,
                self.badge_size,
                self.badge_size.height / 2.0,
                badge_bg,
            );
            if self.badge_text().is_some() {
                let text = self.badge.size();
                self.badge.paint(
                    Point::new(
                        pill.x + (self.badge_size.width - text.width) / 2.0,
                        pill.y + (self.badge_size.height - text.height) / 2.0,
                    ),
                    badge_fg,
                    scene,
                );
            }
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
                // A destination advertises itself as clickable; there is no
                // press/hover chrome to update (upstream suppresses ink
                // outright — see the module docs' Not ported).
                if inside(p.position, ctx.size()) {
                    ctx.set_cursor(CursorIcon::Pointer);
                }
                if self.captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
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
                // A `Cancel` arm only clears internal flags — never state.
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Tab, |node| {
            let label = self
                .semantic_label
                .as_deref()
                .unwrap_or(&self.label.content);
            node.set_label(label);
            node.set_selected(self.selected);
        });
    }

    visit_children!(icon);
}

// ---- The rail --------------------------------------------------------------

/// One section's resolved position in the rail's flat destination list, plus
/// its (expanded-only) header run.
struct SectionLayout {
    /// Flat index of this section's first destination.
    first: usize,
    /// How many destinations this section holds.
    count: usize,
    header: Option<TextRun>,
    /// Where the header paints, in rail-local space (set in `layout`).
    header_origin: Point,
}

/// The retained widget for a [`NavigationRailView`]. See the
/// [module docs](self).
pub struct NavigationRailWidget {
    /// Every destination, flattened across sections — each pod wraps a
    /// `RailItemWidget`.
    items: Vec<ChildPod>,
    menu: Option<ChildPod>,
    fab: Option<ChildPod>,
    trailing: Option<ChildPod>,
    sections: Vec<SectionLayout>,

    selected: usize,
    expanded: bool,
    modal: bool,
    trailing_at_bottom: bool,
    background: Option<Color>,
    on_dismiss_modal: Option<ErasedCallback>,

    width_motion: WidthMotion,
    lead: TravelAxis,
    trail: TravelAxis,
    /// Whether the indicator has ever been measured against a real layout.
    ready: bool,
    /// A selection change waiting for the next layout to resolve into a travel.
    pending_travel: bool,
    /// Cross-axis center / cross size / resting main extent of the selected
    /// destination's indicator, from the last layout.
    indicator_cross: (f64, f64),
    indicator_extent: f64,
    /// Whether the liquid pill is mid-travel (the rail paints it and every
    /// item's own resting fill stays off).
    traveling: bool,

    /// The rail panel's laid-out size (the animated width × the full height).
    panel: Size,
    /// The whole area the widget was given — equal to [`Self::panel`] outside
    /// modal presentation.
    area: Size,
    /// Whether a scrim press is armed (fire-on-up-inside, like every other
    /// press in this catalog).
    scrim_armed: bool,
}

impl NavigationRailWidget {
    /// Whether the modal scrim paints at all this frame — it fades in and out
    /// on the width ramp, so it outlives the expansion by exactly one
    /// transition.
    fn scrim_visible(&self) -> bool {
        self.modal && self.expansion() > 0.0
    }

    /// Whether the scrim currently *blocks* input. Gated on the expansion prop
    /// rather than on the fading paint, matching upstream's
    /// `IgnorePointer(ignoring: !_isExpanded)`: a scrim on its way out stops
    /// swallowing presses immediately, it does not hold the page inert for the
    /// length of its own fade.
    fn barrier_active(&self) -> bool {
        self.modal && self.expanded
    }

    /// How far the rail currently reads as expanded, `0..=1` — derived from
    /// the animated width so the scrim fades on exactly the width ramp
    /// (upstream's `AnimatedContainer` scrim alpha, on the same duration and
    /// curve).
    fn expansion(&self) -> f64 {
        // The ramp's own two endpoints, whichever direction it runs.
        let from = self.width_motion.tween.lerp(0.0);
        let to = self.width_motion.target;
        let (lo, hi) = (from.min(to), from.max(to));
        if hi - lo <= f64::EPSILON {
            return if self.expanded { 1.0 } else { 0.0 };
        }
        ((self.width_motion.value - lo) / (hi - lo)).clamp(0.0, 1.0)
    }

    /// The selected destination's indicator rect in rail-local space, if the
    /// last layout produced one.
    ///
    /// `&mut self` because [`ChildPod`]'s downcast seam is mutable-only
    /// (`dyn Widget::downcast_mut`); nothing here mutates.
    fn selected_indicator(&mut self) -> Option<Rect> {
        let pod = self.items.get_mut(self.selected)?;
        let origin = pod.origin();
        let rect = pod.widget_mut().downcast_mut::<RailItemWidget>()?.indicator;
        Some(Rect::new(
            origin.x + rect.x0,
            origin.y + rect.y0,
            origin.x + rect.x1,
            origin.y + rect.y1,
        ))
    }

    /// Resolve the liquid indicator against the layout that just ran: a first
    /// measure and a width transition **jump**, a selection change travels.
    fn sync_indicator(&mut self) {
        let Some(rect) = self.selected_indicator() else {
            return;
        };
        self.indicator_cross = (rect.center().x, rect.width());
        self.indicator_extent = rect.height();
        let center = rect.center().y;
        if !self.ready || self.width_motion.is_animating() {
            self.lead.jump(center);
            self.trail.jump(center);
            self.ready = true;
            self.pending_travel = false;
            return;
        }
        if self.pending_travel {
            self.lead.travel_to(center, RAIL_LEAD_SPRING);
            self.trail.travel_to(center, RAIL_TRAIL_SPRING);
            self.pending_travel = false;
        }
    }

    /// The liquid pill's rect for the current lead/trail positions — the
    /// reference's `_buildPill` math on the vertical axis.
    fn pill_rect(&self) -> Rect {
        let (cross_center, cross_size) = self.indicator_cross;
        let min_main = self.lead.value.min(self.trail.value);
        let max_main = self.lead.value.max(self.trail.value);
        let extent = (max_main - min_main) + self.indicator_extent;
        let start = min_main - self.indicator_extent / 2.0;
        Rect::new(
            cross_center - cross_size / 2.0,
            start,
            cross_center + cross_size / 2.0,
            start + extent,
        )
    }

    /// Route to whichever slot owns the event, in a fixed order so a captured
    /// gesture in a slot pod always wins over a hit test elsewhere.
    fn route_slots(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            // A broadcast reaches every child unconditionally, ahead of every
            // other branch, and is never consumed.
            if let Some(pod) = self.menu.as_mut() {
                route_event_single(pod, ctx, event);
            }
            if let Some(pod) = self.fab.as_mut() {
                route_event_single(pod, ctx, event);
            }
            if let Some(pod) = self.trailing.as_mut() {
                route_event_single(pod, ctx, event);
            }
            route_event(&mut self.items, ctx, event);
            return EventResult::Ignored;
        }
        for slot in [&mut self.menu, &mut self.fab, &mut self.trailing] {
            if let Some(pod) = slot.as_mut()
                && route_event_single(pod, ctx, event) == EventResult::Handled
            {
                return EventResult::Handled;
            }
        }
        route_event(&mut self.items, ctx, event)
    }
}

impl<State: 'static> View<State> for NavigationRailView<State> {
    type Element = NavigationRailWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NavigationRailWidget {
        let expanded = self.rail_type.is_expanded();
        let mut items = Vec::new();
        let mut sections = Vec::new();
        for section in &self.sections {
            sections.push(SectionLayout {
                first: items.len(),
                count: section.destinations.len(),
                header: section.header.as_ref().map(TextRun::new),
                header_origin: Point::ZERO,
            });
            for destination in &section.destinations {
                let index = items.len();
                items.push(build_item(
                    destination,
                    index == self.selected,
                    expanded,
                    self.label_behavior,
                    &self.on_select,
                    index,
                    ctx,
                ));
            }
        }

        NavigationRailWidget {
            items,
            menu: self.menu_view().map(|view| build_child(&view, ctx)),
            fab: self.fab_view().map(|view| build_child(&view, ctx)),
            trailing: self.trailing.as_ref().map(|view| build_child(view, ctx)),
            sections,
            selected: self.selected,
            expanded,
            modal: self.modality == NavigationRailModality::Modal,
            trailing_at_bottom: self.trailing_at_bottom,
            background: self.background,
            on_dismiss_modal: self.on_dismiss_modal.as_ref().map(erase_callback),
            width_motion: WidthMotion::resting(self.target_width()),
            lead: TravelAxis::new(),
            trail: TravelAxis::new(),
            ready: false,
            pending_travel: false,
            indicator_cross: (0.0, 0.0),
            indicator_extent: 0.0,
            traveling: false,
            panel: Size::ZERO,
            area: Size::ZERO,
            scrim_armed: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut NavigationRailWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let expanded = self.rail_type.is_expanded();
        let was_expanded = prev.rail_type.is_expanded();

        element.on_dismiss_modal = self.on_dismiss_modal.as_ref().map(erase_callback);
        element.trailing_at_bottom = self.trailing_at_bottom;

        if prev.label_behavior != self.label_behavior {
            // Every destination picks the new behavior up in the per-item pass
            // below; a collapsed label appearing or leaving is a size change.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.background != self.background {
            element.background = self.background;
            flags |= ChangeFlags::PAINT;
        }
        if element.modal != (self.modality == NavigationRailModality::Modal) {
            element.modal = self.modality == NavigationRailModality::Modal;
            element.scrim_armed = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.expanded != expanded {
            element.expanded = expanded;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let target = self.target_width();
        if (element.width_motion.target - target).abs() > f64::EPSILON {
            element.width_motion.set_target(target);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.selected != self.selected {
            element.selected = self.selected;
            element.pending_travel = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // ---- Destinations ---------------------------------------------------
        let prev_dests = prev.destinations();
        let next_dests = self.destinations();
        let common = prev_dests.len().min(next_dests.len());
        for index in 0..common {
            let was_selected = index == prev.selected;
            let now_selected = index == self.selected;
            let pod = &mut element.items[index];
            let widget = pod
                .widget_mut()
                .downcast_mut::<RailItemWidget>()
                .expect("a rail destination pod holds a RailItemWidget");
            flags |= rebuild_child(
                prev_dests[index].active_icon(was_selected),
                next_dests[index].active_icon(now_selected),
                &mut widget.icon,
                ctx,
            );
            if widget.label.set_content(&next_dests[index].label) {
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            if widget.badge_count != next_dests[index].badge_count {
                widget.badge_count = next_dests[index].badge_count;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            widget.semantic_label = next_dests[index].semantic_label.clone();
            if widget.selected != now_selected {
                widget.selected = now_selected;
                flags |= ChangeFlags::PAINT;
            }
            if widget.expanded != expanded {
                widget.expanded = expanded;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            widget.label_behavior = self.label_behavior;
            // Closures aren't comparable — always reinstall the adapter.
            widget.on_select = item_on_select::<State>(&self.on_select, index);
        }

        if next_dests.len() > prev_dests.len() {
            for (offset, destination) in next_dests[common..].iter().enumerate() {
                let index = common + offset;
                element.items.push(build_item(
                    destination,
                    index == self.selected,
                    expanded,
                    self.label_behavior,
                    &self.on_select,
                    index,
                    ctx,
                ));
            }
            element.ready = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if next_dests.len() < prev_dests.len() {
            for (offset, destination) in prev_dests[common..].iter().enumerate() {
                let index = common + offset;
                let pod = &mut element.items[index];
                if pod.is_active() {
                    pod.set_active(false);
                }
                let widget = pod
                    .widget_mut()
                    .downcast_mut::<RailItemWidget>()
                    .expect("a rail destination pod holds a RailItemWidget");
                teardown_child(
                    destination.active_icon(index == prev.selected),
                    &mut widget.icon,
                    ctx,
                );
            }
            element.items.truncate(common);
            element.ready = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // ---- Sections -------------------------------------------------------
        let sections_changed = prev.sections.len() != self.sections.len()
            || prev
                .sections
                .iter()
                .zip(self.sections.iter())
                .any(|(a, b)| a.destinations.len() != b.destinations.len() || a.header != b.header);
        if sections_changed {
            let mut first = 0;
            element.sections = self
                .sections
                .iter()
                .map(|section| {
                    let layout = SectionLayout {
                        first,
                        count: section.destinations.len(),
                        header: section.header.as_ref().map(TextRun::new),
                        header_origin: Point::ZERO,
                    };
                    first += section.destinations.len();
                    layout
                })
                .collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // ---- Slots ----------------------------------------------------------
        let prev_menu = prev.menu_view();
        let next_menu = self.menu_view();
        flags |= rebuild_slot(&prev_menu, &next_menu, &mut element.menu, ctx);
        // The menu button's glyph flips with the type, which `rebuild_child`
        // cannot see through a rebuilt icon view of the same shape.
        if was_expanded != expanded {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let fab_changed = match (&prev.fab, &self.fab) {
            (Some(a), Some(b)) => a.shape() != b.shape() || was_expanded != expanded,
            (a, b) => a.is_some() != b.is_some(),
        };
        let prev_fab = prev.fab_view();
        let next_fab = self.fab_view();
        flags |= rebuild_slot(&prev_fab, &next_fab, &mut element.fab, ctx);
        if fab_changed {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= rebuild_slot(&prev.trailing, &self.trailing, &mut element.trailing, ctx);

        flags
    }

    fn teardown(&self, element: &mut NavigationRailWidget, ctx: &mut BuildCtx<'_>) {
        for (index, destination) in self.destinations().iter().enumerate() {
            let Some(pod) = element.items.get_mut(index) else {
                continue;
            };
            let widget = pod
                .widget_mut()
                .downcast_mut::<RailItemWidget>()
                .expect("a rail destination pod holds a RailItemWidget");
            teardown_child(
                destination.active_icon(index == self.selected),
                &mut widget.icon,
                ctx,
            );
        }
        if let (Some(view), Some(pod)) = (self.menu_view(), element.menu.as_mut()) {
            teardown_child(&view, pod, ctx);
        }
        if let (Some(view), Some(pod)) = (self.fab_view(), element.fab.as_mut()) {
            teardown_child(&view, pod, ctx);
        }
        if let (Some(view), Some(pod)) = (self.trailing.as_ref(), element.trailing.as_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// Build/rebuild/tear down an optional slot pod against an optional view — the
/// four-way presence match every slot in this module takes.
fn rebuild_slot<State: 'static>(
    prev: &Option<AnyView<State>>,
    next: &Option<AnyView<State>>,
    pod: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (Some(prev_view), Some(next_view)) => match pod.as_mut() {
            Some(pod) => rebuild_child(prev_view, next_view, pod, ctx),
            None => {
                *pod = Some(build_child(next_view, ctx));
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        },
        (None, Some(next_view)) => {
            *pod = Some(build_child(next_view, ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(prev_view), None) => {
            if let Some(mut old) = pod.take() {
                teardown_child(prev_view, &mut old, ctx);
            }
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (None, None) => ChangeFlags::NONE,
    }
}

impl Widget for NavigationRailWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        let panel_width = self.width_motion.value.max(0.0).min(area.width.max(0.0));
        self.area = area;
        self.panel = Size::new(panel_width, area.height);

        let inner = (panel_width - HORIZONTAL_INSET * 2.0).max(0.0);
        let mut y = TOP_GAP;

        if let Some(pod) = self.menu.as_mut() {
            let size =
                pod.layout_child(ctx, &BoxConstraints::loose(Size::new(inner, f64::INFINITY)));
            let x = if self.expanded {
                HORIZONTAL_INSET
            } else {
                HORIZONTAL_INSET + (inner - size.width) / 2.0
            };
            pod.set_origin(Point::new(x.max(0.0), y));
            y += size.height + SECTION_PADDING_BOTTOM;
        }

        if let Some(pod) = self.fab.as_mut() {
            // Laid out **loose** so the FAB keeps its own token size: upstream's
            // `ListView` hands its children tight cross-axis constraints, which
            // stretches an `M3EFab`'s fixed square across the rail; a rail FAB
            // is a fixed 56dp square (or a pill when extended) here, aligned to
            // the leading edge while expanded and centered while collapsed.
            let size =
                pod.layout_child(ctx, &BoxConstraints::loose(Size::new(inner, f64::INFINITY)));
            let x = if self.expanded {
                HORIZONTAL_INSET
            } else {
                HORIZONTAL_INSET + (inner - size.width) / 2.0
            };
            pod.set_origin(Point::new(x.max(0.0), y));
            y += size.height + SECTION_PADDING_BOTTOM;
        }

        let expanded = self.expanded;
        let mut sections = std::mem::take(&mut self.sections);
        for section in &mut sections {
            if expanded && let Some(header) = section.header.as_mut() {
                let theme = Theme::from_layout_ctx(ctx);
                let style = type_style(theme.map(|t| &t.type_scale.title_small), TITLE_SMALL);
                y += SECTION_HEADER_SPACING_TOP;
                let size = header.shape(ctx, &style, Some(inner));
                section.header_origin = Point::new(HORIZONTAL_INSET, y);
                y += size.height + SECTION_HEADER_SPACING_BOTTOM;
            }
            for index in section.first..section.first + section.count {
                let Some(pod) = self.items.get_mut(index) else {
                    continue;
                };
                y += ITEM_VERTICAL_GAP;
                let size = pod.layout_child(
                    ctx,
                    &BoxConstraints::new(Size::new(inner, 0.0), Size::new(inner, f64::INFINITY)),
                );
                pod.set_origin(Point::new(HORIZONTAL_INSET, y));
                y += size.height + ITEM_VERTICAL_GAP;
            }
        }
        self.sections = sections;

        if let Some(pod) = self.trailing.as_mut() {
            let size =
                pod.layout_child(ctx, &BoxConstraints::loose(Size::new(inner, f64::INFINITY)));
            let x = if expanded {
                HORIZONTAL_INSET
            } else {
                HORIZONTAL_INSET + (inner - size.width) / 2.0
            };
            let top = if self.trailing_at_bottom {
                (area.height - SECTION_PADDING_BOTTOM - size.height).max(y)
            } else {
                y
            };
            pod.set_origin(Point::new(x.max(0.0), top));
        }

        self.sync_indicator();

        // Modal presentation fills the area it was given (the scrim covers
        // everything the panel does not); standard presentation is exactly its
        // own panel.
        let size = if self.modal { area } else { self.panel };
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let container = self
            .background
            .unwrap_or_else(|| theme.map_or(SURFACE, |t| t.scheme().surface));
        let scrim_base = theme.map_or(SCRIM, |t| t.scheme().scrim);
        let indicator_fill = theme.map_or(SECONDARY_CONTAINER, |t| t.scheme().secondary_container);
        let header_ink = theme.map_or(ON_SURFACE_VARIANT, |t| t.scheme().on_surface_variant);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();

        // The rail's width is a layout value, so an in-flight ramp needs an
        // explicit relayout request, not merely another frame (see the module
        // docs' layout-skip section). A reduce-motion snap that actually moved
        // the width needs exactly one.
        let width_changed = if reduce_motion {
            self.width_motion.snap()
        } else {
            let before = self.width_motion.value;
            // Either arm asks: still in flight (the next frame moves it again),
            // or the value moved this frame — the *settling* frame is the one
            // that both stops animating and lands on the target, so gating on
            // the animating flag alone would leave the rail a fraction of a
            // pixel short of its resting width until some later relayout.
            self.width_motion.advance(now) || self.width_motion.value != before
        };
        if width_changed {
            ctx.request_layout();
        }

        // The liquid pill is paint-only — the items it travels between do not
        // move — so it asks for a frame, never a relayout.
        if reduce_motion {
            self.lead.jump(self.lead.target);
            self.trail.jump(self.trail.target);
        } else {
            let lead = self.lead.advance(now);
            let trail = self.trail.advance(now);
            if lead || trail {
                ctx.request_frame();
            }
        }
        self.traveling = self.lead.is_animating() || self.trail.is_animating();

        let origin = ctx.origin();
        if self.scrim_visible() {
            let alpha = OVERLAY_SCRIM_ALPHA * self.expansion() as f32;
            scene.fill_rect(origin, self.area, with_alpha(scrim_base, alpha));
        }

        if self.panel.width > 0.0 {
            scene.fill_rect(origin, self.panel, container);
        }

        if self.traveling && self.ready {
            let pill = self.pill_rect();
            scene.fill_rounded_rect(
                Point::new(origin.x + pill.x0, origin.y + pill.y0),
                pill.size(),
                pill.width().min(pill.height()) / 2.0,
                indicator_fill,
            );
        }

        // Top-to-bottom, the order `visit_children!` and `semantics` both
        // report: the header slots, the destinations (with their section
        // headers), then the trailing slot.
        for slot in [&mut self.menu, &mut self.fab] {
            if let Some(pod) = slot.as_mut() {
                pod.paint_child(ctx, scene);
            }
        }
        let traveling = self.traveling;
        for pod in &mut self.items {
            if let Some(widget) = pod.widget_mut().downcast_mut::<RailItemWidget>() {
                widget.use_local_indicator = !traveling;
            }
            pod.paint_child(ctx, scene);
        }
        if self.expanded {
            for section in &self.sections {
                if let Some(header) = &section.header {
                    header.paint(
                        Point::new(
                            origin.x + section.header_origin.x,
                            origin.y + section.header_origin.y,
                        ),
                        header_ink,
                        scene,
                    );
                }
            }
        }
        if let Some(pod) = self.trailing.as_mut() {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A broadcast is not user input: it reaches every child ahead of every
        // other branch and is never consumed.
        if event.is_broadcast() {
            return self.route_slots(ctx, event);
        }
        let InputEvent::Pointer(p) = event else {
            return self.route_slots(ctx, event);
        };
        if !self.barrier_active() {
            return self.route_slots(ctx, event);
        }
        if p.position.x < self.panel.width {
            // Inside the panel: its own content owns the event, and whatever
            // the content declines is swallowed rather than falling through —
            // a modal rail blocks the page behind it, the same barrier
            // contract `crate::overlay`'s modal host holds.
            self.route_slots(ctx, event);
            return EventResult::Handled;
        }
        // On the scrim: the barrier swallows every pointer whatever button
        // pressed it, but only the primary one arms a dismiss (the catalog's
        // press rule).
        match p.phase {
            PointerPhase::Down => {
                if presses(p) {
                    self.scrim_armed = true;
                    ctx.capture_pointer();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if self.scrim_armed
                    && p.position.x >= self.panel.width
                    && let Some(on_dismiss) = self.on_dismiss_modal.as_mut()
                {
                    (on_dismiss)(ctx);
                }
                self.scrim_armed = false;
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // A `Cancel` arm only clears internal flags — never state.
                self.scrim_armed = false;
                EventResult::Handled
            }
            PointerPhase::Move => EventResult::Handled,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                if let Some(pod) = &self.menu {
                    pod.semantics_child(ctx);
                }
                if let Some(pod) = &self.fab {
                    pod.semantics_child(ctx);
                }
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
                if let Some(pod) = &self.trailing {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(menu, fab, items, trailing);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    const AREA: Size = Size::new(400.0, 600.0);

    fn dest<State: 'static>(label: &str) -> RailDestination<State> {
        rail_destination(any(icon(crate::icons::HOME)), label)
    }

    fn one_section<State: 'static>() -> Vec<RailSection<State>> {
        vec![rail_section(vec![
            dest("Home"),
            dest("Search"),
            dest("Profile"),
        ])]
    }

    fn rail<State: 'static>() -> NavigationRailView<State> {
        navigation_rail(one_section(), 0, |_state: &mut State, _index| {})
    }

    fn build<State: 'static>(view: &NavigationRailView<State>) -> NavigationRailWidget {
        let mut counter = 0u64;
        View::<State>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild<State: 'static>(
        next: &NavigationRailView<State>,
        prev: &NavigationRailView<State>,
        widget: &mut NavigationRailWidget,
    ) -> ChangeFlags {
        let mut counter = 1u64;
        View::<State>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut counter))
    }

    fn layout_themed(widget: &mut NavigationRailWidget, theme: Option<&Theme>, area: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        widget.layout(&mut ctx, &BoxConstraints::loose(area))
    }

    fn layout(widget: &mut NavigationRailWidget) -> Size {
        layout_themed(widget, None, AREA)
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    /// Paint at `t_ms` with a real clock, returning `(recorder, needs_layout)`.
    fn paint_at(
        widget: &mut NavigationRailWidget,
        t_ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx =
            PaintCtx::for_test(Point::ZERO, AREA, FrameTime::from_nanos(t_ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        widget.paint(&mut ctx, &mut rec);
        let needs_layout = ctx.needs_layout();
        (rec, needs_layout)
    }

    fn dispatch(
        widget: &mut NavigationRailWidget,
        state: &mut dyn Any,
        phase: PointerPhase,
        pos: Point,
    ) -> EventResult {
        let event = InputEvent::Pointer(PointerEvent {
            phase,
            position: pos,
            button: PointerButton::Primary,
        });
        let mut ctx = EventCtx::new(state, Point::ZERO, AREA);
        widget.event(&mut ctx, &event)
    }

    fn item(widget: &mut NavigationRailWidget, index: usize) -> &mut RailItemWidget {
        widget.items[index]
            .widget_mut()
            .downcast_mut::<RailItemWidget>()
            .expect("a rail destination pod holds a RailItemWidget")
    }

    /// Press and release inside `pos`, the catalog's fire-on-up-inside path.
    fn tap(widget: &mut NavigationRailWidget, state: &mut dyn Any, pos: Point) {
        dispatch(widget, state, PointerPhase::Down, pos);
        dispatch(widget, state, PointerPhase::Up, pos);
    }

    // ---- Geometry ---------------------------------------------------------

    #[test]
    fn collapsed_geometry_pins_the_rail_width_and_its_item_boxes() {
        let view: NavigationRailView<()> = rail().rail_type(NavigationRailType::Collapsed);
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(
            size,
            Size::new(RAIL_COLLAPSED_WIDTH, AREA.height),
            "a standard rail is exactly its own width by the height it was given"
        );

        let inner = RAIL_COLLAPSED_WIDTH - HORIZONTAL_INSET * 2.0;
        for index in 0..3 {
            let pod = &w.items[index];
            assert_eq!(pod.origin().x, HORIZONTAL_INSET);
            assert_eq!(pod.size(), Size::new(inner, RAIL_ITEM_COLLAPSED_HEIGHT));
        }
        // Adjacent destinations are separated by the gap on both their edges.
        let spacing = w.items[1].origin().y - w.items[0].origin().y;
        assert_eq!(
            spacing,
            RAIL_ITEM_COLLAPSED_HEIGHT + ITEM_VERTICAL_GAP * 2.0
        );

        // The menu button sits above the first destination, under the top gap.
        let menu = w
            .menu
            .as_ref()
            .expect("a collapsed rail carries a menu button");
        assert_eq!(menu.origin().y, TOP_GAP);
        assert!(w.items[0].origin().y > menu.origin().y + menu.size().height);

        // The collapsed indicator is the icon chip, not the whole item.
        let chip = item(&mut w, 0).indicator;
        assert_eq!(chip.x0, 0.0);
        assert_eq!(chip.width(), CHIP_WIDTH);
        assert_eq!(chip.height(), CHIP_HEIGHT);
    }

    #[test]
    fn expanded_geometry_pins_the_rail_width_and_makes_the_row_the_indicator() {
        let view: NavigationRailView<()> = rail().rail_type(NavigationRailType::Expanded);
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size, Size::new(RAIL_EXPANDED_MIN_WIDTH, AREA.height));

        let inner = RAIL_EXPANDED_MIN_WIDTH - HORIZONTAL_INSET * 2.0;
        let pod = &w.items[0];
        assert_eq!(pod.origin().x, HORIZONTAL_INSET);
        assert_eq!(pod.size(), Size::new(inner, RAIL_ITEM_EXPANDED_HEIGHT));

        let widget = item(&mut w, 0);
        assert_eq!(
            widget.indicator,
            Rect::new(0.0, 0.0, inner, RAIL_ITEM_EXPANDED_HEIGHT),
            "an expanded row *is* its own selection indicator"
        );
        // Icon then label, both inside the row's leading inset.
        assert_eq!(widget.icon.origin().x, INDICATOR_LEADING);
        assert_eq!(
            widget.label_origin.x,
            INDICATOR_LEADING + ICON_SIZE + ICON_LABEL_GAP
        );
    }

    #[test]
    fn the_expanded_width_clamps_to_the_reference_band() {
        let wide: NavigationRailView<()> = rail()
            .rail_type(NavigationRailType::Expanded)
            .expanded_width(1000.0);
        assert_eq!(wide.target_width(), RAIL_EXPANDED_MAX_WIDTH);
        let narrow: NavigationRailView<()> = rail()
            .rail_type(NavigationRailType::Expanded)
            .expanded_width(10.0);
        assert_eq!(narrow.target_width(), RAIL_EXPANDED_MIN_WIDTH);
        let default: NavigationRailView<()> = rail().rail_type(NavigationRailType::Expanded);
        assert_eq!(default.target_width(), RAIL_EXPANDED_MIN_WIDTH);
    }

    #[test]
    fn hide_when_collapsed_collapses_the_rail_to_zero_width() {
        let view: NavigationRailView<()> = rail()
            .rail_type(NavigationRailType::Collapsed)
            .hide_when_collapsed(true);
        let mut w = build(&view);
        assert_eq!(layout(&mut w), Size::new(0.0, AREA.height));
    }

    #[test]
    fn trailing_pins_to_the_bottom_edge_by_default_and_follows_the_items_otherwise() {
        let bottom: NavigationRailView<()> = rail()
            .rail_type(NavigationRailType::Expanded)
            .trailing(any(icon(crate::icons::SEARCH)));
        let mut w = build(&bottom);
        layout(&mut w);
        let pod = w.trailing.as_ref().expect("a trailing slot");
        assert_eq!(
            pod.origin().y + pod.size().height + SECTION_PADDING_BOTTOM,
            AREA.height
        );

        let inline: NavigationRailView<()> = rail()
            .rail_type(NavigationRailType::Expanded)
            .trailing(any(icon(crate::icons::SEARCH)))
            .trailing_at_bottom(false);
        let mut w = build(&inline);
        layout(&mut w);
        let last_item = w.items.last().expect("three destinations");
        let bottom_of_items = last_item.origin().y + last_item.size().height;
        let pod = w.trailing.as_ref().expect("a trailing slot");
        assert!(pod.origin().y < bottom_of_items + RAIL_ITEM_EXPANDED_HEIGHT);
    }

    // ---- The width transition ---------------------------------------------

    #[test]
    fn a_type_change_ramps_the_width_and_paint_requests_layout_every_frame() {
        // The regression guard for the layout-skip trap: the rail's width is
        // computed in `layout` from a value advanced in `paint`, so every
        // in-flight frame must ask for a relayout — a paint-only frame request
        // would freeze the rail mid-ramp under the mobile intra-frame layout
        // skip.
        let collapsed: NavigationRailView<()> = rail().rail_type(NavigationRailType::Collapsed);
        let mut w = build(&collapsed);
        assert_eq!(layout(&mut w).width, RAIL_COLLAPSED_WIDTH);
        let (_, settled) = paint_at(&mut w, 0, None);
        assert!(!settled, "a resting rail requests no layout");

        let expanded: NavigationRailView<()> = rail().rail_type(NavigationRailType::Expanded);
        let flags = rebuild(&expanded, &collapsed, &mut w);
        assert!(flags.needs_layout());

        let mut widths = vec![layout(&mut w).width];
        let mut t_ms = 0u64;
        let mut frames = 0u32;
        loop {
            let (_, needs_layout) = paint_at(&mut w, t_ms, None);
            if !needs_layout {
                break;
            }
            frames += 1;
            // A real shell relayouts on seeing `needs_layout`.
            widths.push(layout(&mut w).width);
            t_ms += 16;
            assert!(frames < 200, "a 280ms ramp settles well inside 200 frames");
        }
        assert!(frames > 1, "the ramp spans more than one frame");
        assert!(
            widths
                .windows(2)
                .all(|pair| pair[1] >= pair[0] - f64::EPSILON),
            "the width rises monotonically: {widths:?}"
        );
        assert_eq!(
            widths.last().copied(),
            Some(RAIL_EXPANDED_MIN_WIDTH),
            "the settled ramp lands exactly on the expanded width"
        );
        let (_, still) = paint_at(&mut w, t_ms + 16, None);
        assert!(!still, "settled: no more layout requests");
    }

    #[test]
    fn reduce_motion_snaps_the_width_and_requests_exactly_one_relayout() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let collapsed: NavigationRailView<()> = rail().rail_type(NavigationRailType::Collapsed);
        let mut w = build(&collapsed);
        layout_themed(&mut w, Some(&theme), AREA);

        let expanded: NavigationRailView<()> = rail().rail_type(NavigationRailType::Expanded);
        rebuild(&expanded, &collapsed, &mut w);

        let (_, needs_layout) = paint_at(&mut w, 0, Some(&theme));
        assert_eq!(w.width_motion.value, RAIL_EXPANDED_MIN_WIDTH, "snapped");
        assert!(
            needs_layout,
            "the snap changed a layout value, so it must request one relayout"
        );
        assert_eq!(
            layout_themed(&mut w, Some(&theme), AREA).width,
            RAIL_EXPANDED_MIN_WIDTH,
            "the rail reaches its target on the snap frame, not some later one"
        );
        let (_, still) = paint_at(&mut w, 16, Some(&theme));
        assert!(
            !still,
            "a snap requests exactly one relayout, never an ongoing frame request"
        );
    }

    // ---- Controlled semantics ---------------------------------------------

    #[test]
    fn a_tap_on_a_destination_reports_its_flat_index_without_self_mutating() {
        let view: NavigationRailView<Vec<usize>> = navigation_rail(
            vec![
                rail_section(vec![dest("Home"), dest("Search")]),
                rail_section(vec![dest("Profile")]).header("Account"),
            ],
            0,
            |state: &mut Vec<usize>, index| state.push(index),
        )
        .rail_type(NavigationRailType::Expanded);
        let mut w = build(&view);
        layout(&mut w);

        let mut log: Vec<usize> = Vec::new();
        // The third destination lives in the *second* section: the reported
        // index is flat across sections, like upstream's own.
        let pod = &w.items[2];
        let pos = Point::new(
            pod.origin().x + 10.0,
            pod.origin().y + pod.size().height / 2.0,
        );
        tap(&mut w, &mut log, pos);
        assert_eq!(log, vec![2]);
        assert_eq!(w.selected, 0, "the rail never moves its own selection");
    }

    #[test]
    fn the_menu_button_reports_the_toggled_type_without_self_mutating() {
        let view: NavigationRailView<Vec<NavigationRailType>> = navigation_rail(
            one_section(),
            0,
            |_state: &mut Vec<NavigationRailType>, _index| {},
        )
        .rail_type(NavigationRailType::Expanded)
        .on_type_changed(|state: &mut Vec<NavigationRailType>, next| state.push(next));
        let mut w = build(&view);
        layout(&mut w);

        let mut log: Vec<NavigationRailType> = Vec::new();
        let pod = w
            .menu
            .as_ref()
            .expect("an expanded rail carries a menu button");
        let pos = Point::new(
            pod.origin().x + pod.size().width / 2.0,
            pod.origin().y + pod.size().height / 2.0,
        );
        tap(&mut w, &mut log, pos);
        assert_eq!(log, vec![NavigationRailType::Collapsed]);
        assert!(w.expanded, "the rail never collapses itself");
    }

    #[test]
    fn the_always_variants_carry_no_menu_button() {
        for rail_type in [
            NavigationRailType::AlwaysExpand,
            NavigationRailType::AlwaysCollapse,
        ] {
            let view: NavigationRailView<()> = rail().rail_type(rail_type);
            let w = build(&view);
            assert!(w.menu.is_none(), "{rail_type:?} offers no toggle");
        }
        for rail_type in [NavigationRailType::Expanded, NavigationRailType::Collapsed] {
            let view: NavigationRailView<()> = rail().rail_type(rail_type);
            let w = build(&view);
            assert!(w.menu.is_some(), "{rail_type:?} offers a toggle");
        }
    }

    // ---- Modal presentation -----------------------------------------------

    #[test]
    fn a_modal_rail_fills_its_area_and_paints_a_scrim_beside_its_panel() {
        let view: NavigationRailView<()> = rail()
            .rail_type(NavigationRailType::Expanded)
            .modality(NavigationRailModality::Modal);
        let mut w = build(&view);
        assert_eq!(
            layout(&mut w),
            AREA,
            "a modal rail is a full-area host, not a layout column"
        );
        let (rec, _) = paint_at(&mut w, 0, None);
        let scrim = rec
            .rects
            .iter()
            .find(|(_, size, _)| *size == AREA)
            .expect("the scrim covers the whole area");
        assert!(
            (scrim.2.components[3] - OVERLAY_SCRIM_ALPHA).abs() < 1e-6,
            "the scrim paints at the M3 32%"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(_, size, _)| *size == Size::new(RAIL_EXPANDED_MIN_WIDTH, AREA.height)),
            "the panel is pinned to the leading edge at the rail's own width"
        );
        // A standard rail paints no scrim at all.
        let standard: NavigationRailView<()> = rail().rail_type(NavigationRailType::Expanded);
        let mut w = build(&standard);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, 0, None);
        assert!(rec.rects.iter().all(|(_, size, _)| *size != AREA));
    }

    #[test]
    fn a_scrim_press_reports_the_dismiss_request_and_never_reaches_the_page() {
        let view: NavigationRailView<u32> =
            navigation_rail(one_section(), 0, |_state: &mut u32, _index| {})
                .rail_type(NavigationRailType::Expanded)
                .modality(NavigationRailModality::Modal)
                .on_dismiss_modal(|state: &mut u32| *state += 1);
        let mut w = build(&view);
        layout(&mut w);

        let mut dismissed = 0u32;
        let on_scrim = Point::new(RAIL_EXPANDED_MIN_WIDTH + 40.0, 200.0);
        assert_eq!(
            dispatch(&mut w, &mut dismissed, PointerPhase::Down, on_scrim),
            EventResult::Handled,
            "the barrier swallows the press rather than letting the page take it"
        );
        assert_eq!(dismissed, 0, "nothing fires on the way down");
        dispatch(&mut w, &mut dismissed, PointerPhase::Up, on_scrim);
        assert_eq!(dismissed, 1, "fire-on-up-inside, like every other press");

        // A press the platform steals fires nothing.
        dispatch(&mut w, &mut dismissed, PointerPhase::Down, on_scrim);
        dispatch(&mut w, &mut dismissed, PointerPhase::Cancel, on_scrim);
        assert_eq!(dismissed, 1);

        // The panel's own empty space is part of the barrier too: it takes the
        // press without dismissing, rather than letting the page behind it act.
        let on_panel = Point::new(RAIL_EXPANDED_MIN_WIDTH - 4.0, AREA.height - 4.0);
        assert_eq!(
            dispatch(&mut w, &mut dismissed, PointerPhase::Down, on_panel),
            EventResult::Handled
        );
        dispatch(&mut w, &mut dismissed, PointerPhase::Up, on_panel);
        assert_eq!(dismissed, 1, "a press on the panel is not a dismiss");
    }

    #[test]
    fn a_collapsed_modal_rail_lets_presses_outside_its_panel_through() {
        let view: NavigationRailView<u32> =
            navigation_rail(one_section(), 0, |_state: &mut u32, _index| {})
                .rail_type(NavigationRailType::Collapsed)
                .modality(NavigationRailModality::Modal)
                .on_dismiss_modal(|state: &mut u32| *state += 1);
        let mut w = build(&view);
        layout(&mut w);

        let mut dismissed = 0u32;
        let beside = Point::new(RAIL_COLLAPSED_WIDTH + 40.0, 200.0);
        assert_eq!(
            dispatch(&mut w, &mut dismissed, PointerPhase::Down, beside),
            EventResult::Ignored,
            "nothing behind a collapsed modal rail goes inert"
        );
        assert_eq!(dismissed, 0);
    }

    // ---- Sections, FAB, labels --------------------------------------------

    #[test]
    fn a_section_header_takes_space_only_while_expanded() {
        let headed = || {
            vec![
                rail_section(vec![dest::<()>("Home")]).header("Main"),
                rail_section(vec![dest::<()>("Profile")]).header("Account"),
            ]
        };
        let bare = || {
            vec![
                rail_section(vec![dest::<()>("Home")]),
                rail_section(vec![dest::<()>("Profile")]),
            ]
        };

        let mut expanded_headed = build(&navigation_rail(headed(), 0, |_s: &mut (), _i| {}));
        let mut expanded_bare = build(&navigation_rail(bare(), 0, |_s: &mut (), _i| {}));
        layout(&mut expanded_headed);
        layout(&mut expanded_bare);
        assert!(
            expanded_headed.items[0].origin().y > expanded_bare.items[0].origin().y,
            "an expanded rail reserves space above its first destination for the header"
        );
        assert!(
            expanded_headed.sections[1].header_origin.y > 0.0,
            "every section's header is placed"
        );

        let mut collapsed_headed = build(
            &navigation_rail(headed(), 0, |_s: &mut (), _i| {})
                .rail_type(NavigationRailType::Collapsed),
        );
        let mut collapsed_bare = build(
            &navigation_rail(bare(), 0, |_s: &mut (), _i| {})
                .rail_type(NavigationRailType::Collapsed),
        );
        layout(&mut collapsed_headed);
        layout(&mut collapsed_bare);
        assert_eq!(
            collapsed_headed.items[1].origin().y,
            collapsed_bare.items[1].origin().y,
            "a collapsed rail flattens every section and drops the headers"
        );
    }

    #[test]
    fn the_fab_slot_renders_per_type_and_fires() {
        let collapsed: NavigationRailView<u32> =
            navigation_rail(one_section(), 0, |_state: &mut u32, _index| {})
                .rail_type(NavigationRailType::Collapsed)
                .fab(rail_fab(crate::icons::ADD, "Compose", |state: &mut u32| {
                    *state += 1
                }));
        let mut w = build(&collapsed);
        layout(&mut w);
        let pod = w.fab.as_ref().expect("the FAB slot renders");
        let square = pod.size();
        assert_eq!(
            square.width, square.height,
            "a collapsed rail shows the plain square FAB"
        );

        let mut pressed = 0u32;
        let pos = Point::new(
            pod.origin().x + square.width / 2.0,
            pod.origin().y + square.height / 2.0,
        );
        tap(&mut w, &mut pressed, pos);
        assert_eq!(pressed, 1);

        let expanded: NavigationRailView<u32> =
            navigation_rail(one_section(), 0, |_state: &mut u32, _index| {})
                .rail_type(NavigationRailType::Expanded)
                .fab(rail_fab(crate::icons::ADD, "Compose", |state: &mut u32| {
                    *state += 1
                }));
        let mut w = build(&expanded);
        // `extended_fab` grows its label in over its own `MEDIUM_2` ramp on
        // mount (that module's documented v1 mount quirk), so drive frames
        // before measuring the settled pill.
        let mut t_ms = 0u64;
        for _ in 0..40 {
            layout(&mut w);
            paint_at(&mut w, t_ms, None);
            t_ms += 16;
        }
        let pill = w.fab.as_ref().expect("the FAB slot renders").size();
        assert!(
            pill.width > pill.height,
            "an expanded rail shows the extended (labeled) FAB, got {pill:?}"
        );
    }

    #[test]
    fn the_label_behavior_gates_a_collapsed_label_but_never_an_expanded_one() {
        for (behavior, selected_shows, unselected_shows) in [
            (RailLabelBehavior::AlwaysShow, true, true),
            (RailLabelBehavior::OnlySelected, true, false),
            (RailLabelBehavior::AlwaysHide, false, false),
        ] {
            let view: NavigationRailView<()> = rail()
                .rail_type(NavigationRailType::Collapsed)
                .label_behavior(behavior);
            let mut w = build(&view);
            layout(&mut w);
            assert_eq!(item(&mut w, 0).shows_label(), selected_shows);
            assert_eq!(item(&mut w, 1).shows_label(), unselected_shows);

            let view: NavigationRailView<()> = rail()
                .rail_type(NavigationRailType::Expanded)
                .label_behavior(behavior);
            let mut w = build(&view);
            layout(&mut w);
            assert!(
                item(&mut w, 1).shows_label(),
                "an expanded row always shows its label, whatever {behavior:?} says"
            );
        }
    }

    #[test]
    fn a_badge_count_clamps_to_the_documented_three_digit_cap() {
        let view: NavigationRailView<()> = navigation_rail(
            vec![rail_section(vec![
                dest("None"),
                dest("Dot").badge_count(0),
                dest("Small").badge_count(7),
                dest("Huge").badge_count(5_000),
            ])],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = build(&view);
        layout(&mut w);
        assert_eq!(item(&mut w, 0).badge_text(), None);
        assert_eq!(item(&mut w, 1).badge_text(), None, "a zero count is a dot");
        assert_eq!(item(&mut w, 1).badge_size, Size::new(BADGE_DOT, BADGE_DOT));
        assert_eq!(item(&mut w, 2).badge_text().as_deref(), Some("7"));
        assert_eq!(item(&mut w, 3).badge_text().as_deref(), Some("999+"));
        assert_eq!(item(&mut w, 0).badge_size, Size::ZERO);
    }

    // ---- The liquid selection indicator ------------------------------------

    #[test]
    fn the_indicator_springs_match_the_reference_motion() {
        let reference = crate::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT;
        assert_eq!(RAIL_LEAD_SPRING.stiffness, reference.stiffness);
        assert_eq!(RAIL_TRAIL_SPRING.stiffness, reference.stiffness);
        assert_eq!(RAIL_LEAD_SPRING.mass, 1.0);
        assert_eq!(RAIL_TRAIL_SPRING.mass, 1.0);
        // The lead edge is the less damped of the two — that is the stretch.
        assert_eq!(RAIL_LEAD_SPRING.damping_ratio, 0.45);
        assert_eq!(RAIL_TRAIL_SPRING.damping_ratio, 0.55);
    }

    #[test]
    fn an_interrupted_retarget_carries_the_edges_real_velocity() {
        // Regression for the near-zero relaunch bug: before the fix,
        // `travel_to` rebuilt a fresh `AnimationController::fling` at a fixed
        // near-zero launch velocity on every retarget, discarding whatever
        // rate the interrupted leg was actually moving at — a fast
        // double-tap between destinations would visibly pause/kink instead
        // of reversing smoothly.
        let mut axis = TravelAxis::new();
        axis.travel_to(200.0, RAIL_LEAD_SPRING);

        // A few real frames into the first leg, the edge has built up real
        // (well above near-zero) velocity toward its target.
        let mut now = FrameTime::from_nanos(0);
        for step in 1..=3u64 {
            now = FrameTime::from_nanos(step * 16_000_000);
            assert!(axis.advance(now), "the leg should still be in flight");
        }
        let pre_retarget_velocity = axis.velocity;
        assert!(
            pre_retarget_velocity.abs() > 10.0,
            "a few frames into a 200px travel should already be moving well \
             above a near-zero relaunch rate, got {pre_retarget_velocity}"
        );

        // Interrupt with a fresh target before the first leg settles.
        axis.travel_to(0.0, RAIL_LEAD_SPRING);

        // The freshly solved spring's velocity at its own t=0 is exactly its
        // initial condition `v0` — asserting it equals the pre-retarget
        // reading is the direct "carried, not reset" check.
        let carried = axis
            .flight
            .expect("travel_to launches a fresh leg")
            .velocity(0.0);
        assert!(
            (carried - pre_retarget_velocity).abs() < 1e-6,
            "retarget must carry the interrupted leg's real velocity into the \
             new spring's initial condition, got carried={carried} \
             pre_retarget={pre_retarget_velocity}"
        );

        // And the first *moving* post-retarget frame's displacement is
        // consistent with that carried rate, not a near-zero relaunch (a
        // 16ms frame at the near-zero pre-fix launch velocity of 1e-3 would
        // move a vanishing fraction of a pixel). `advance`'s own contract
        // seeds its delta clock on the first call after a motion starts
        // (zero delta, per `AnimationController::advance`'s doc, which
        // `TravelAxis::travel_to` mirrors by clearing `last_time`) — so the
        // seed frame is expected to read the same position, and the frame
        // after it is where the carried rate actually shows.
        axis.advance(now);
        let value_before_retarget = axis.value;
        let next = FrameTime::from_nanos(now.as_nanos() + 16_000_000);
        axis.advance(next);
        let post_retarget_displacement = (axis.value - value_before_retarget).abs();
        assert!(
            post_retarget_displacement > 0.5,
            "the first post-retarget frame should move visibly, matching the \
             carried rate rather than pausing/kinking near zero, got \
             displacement={post_retarget_displacement}"
        );
    }

    #[test]
    fn a_selection_change_travels_the_pill_then_hands_it_back_to_the_item() {
        let first: NavigationRailView<()> = rail().rail_type(NavigationRailType::Expanded);
        let mut w = build(&first);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, 0, None);
        assert!(!w.traveling, "a settled rail paints no bridge");
        assert!(
            item(&mut w, 0).use_local_indicator,
            "the selected item owns its own resting pill"
        );
        let resting = w.items[0].size();
        assert!(
            rec.rrects.iter().any(|(_, size, _, _)| *size == resting),
            "the resting indicator is the selected row itself"
        );

        let moved: NavigationRailView<()> = navigation_rail(one_section(), 2, |_s: &mut (), _i| {})
            .rail_type(NavigationRailType::Expanded);
        rebuild(&moved, &first, &mut w);
        layout(&mut w);

        let start = w.items[0].origin().y + w.items[0].size().height / 2.0;
        let end = w.items[2].origin().y + w.items[2].size().height / 2.0;
        let mut t_ms = 0u64;
        let mut frames = 0u32;
        let mut stretched = false;
        loop {
            paint_at(&mut w, t_ms, None);
            if !w.traveling {
                break;
            }
            assert!(
                !item(&mut w, 2).use_local_indicator,
                "while the bridge travels, no item paints its own fill"
            );
            let pill = w.pill_rect();
            stretched |= pill.height() > w.indicator_extent + 1.0;
            assert!(
                pill.center().y >= start - 40.0 && pill.center().y <= end + 40.0,
                "the bridge stays between the two destinations"
            );
            frames += 1;
            t_ms += 16;
            assert!(frames < 400, "the travel settles well inside 400 frames");
        }
        assert!(frames > 1, "the travel spans more than one frame");
        assert!(
            stretched,
            "the lead runs ahead of the trail — the pill elongates into a bridge"
        );
        assert!(
            (w.pill_rect().center().y - end).abs() < 1e-6,
            "the settled pill lands on the newly selected destination"
        );
        assert!(
            item(&mut w, 2).use_local_indicator,
            "once settled the item takes its resting pill back"
        );
    }

    #[test]
    fn a_width_transition_jumps_the_pill_instead_of_morphing_it() {
        let collapsed: NavigationRailView<()> =
            navigation_rail(one_section(), 1, |_s: &mut (), _i| {})
                .rail_type(NavigationRailType::Collapsed);
        let mut w = build(&collapsed);
        layout(&mut w);

        let expanded: NavigationRailView<()> =
            navigation_rail(one_section(), 1, |_s: &mut (), _i| {})
                .rail_type(NavigationRailType::Expanded);
        rebuild(&expanded, &collapsed, &mut w);

        let mut t_ms = 0u64;
        for _ in 0..40 {
            let (_, needs_layout) = paint_at(&mut w, t_ms, None);
            assert!(
                !w.traveling,
                "a width transition remeasures the pill, it never morphs it"
            );
            if !needs_layout {
                break;
            }
            layout(&mut w);
            let center = w.items[1].origin().y + w.items[1].size().height / 2.0;
            assert!(
                (w.pill_rect().center().y - center).abs() < 1e-6,
                "the pill tracks the selected destination through the ramp"
            );
            t_ms += 16;
        }
    }

    // ---- Reconciliation ----------------------------------------------------

    #[test]
    fn growing_and_shrinking_the_destination_list_reconciles() {
        let two: NavigationRailView<()> = navigation_rail(
            vec![rail_section(vec![dest("Home"), dest("Search")])],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = build(&two);
        layout(&mut w);
        assert_eq!(w.items.len(), 2);

        let three: NavigationRailView<()> = rail();
        let flags = rebuild(&three, &two, &mut w);
        assert_eq!(w.items.len(), 3);
        assert!(flags.needs_layout());

        let one: NavigationRailView<()> = navigation_rail(
            vec![rail_section(vec![dest("Home")])],
            0,
            |_s: &mut (), _i| {},
        );
        let flags = rebuild(&one, &three, &mut w);
        assert_eq!(w.items.len(), 1);
        assert!(flags.needs_layout());
        layout(&mut w);
    }

    #[test]
    fn a_rebuild_adopts_the_new_selection_and_labels() {
        let first: NavigationRailView<()> = rail();
        let mut w = build(&first);
        layout(&mut w);
        assert!(item(&mut w, 0).selected);

        let second: NavigationRailView<()> = navigation_rail(
            vec![rail_section(vec![
                dest("Home"),
                dest("Explore"),
                dest("Profile"),
            ])],
            1,
            |_s: &mut (), _i| {},
        );
        let flags = rebuild(&second, &first, &mut w);
        assert!(flags.needs_paint());
        assert!(!item(&mut w, 0).selected);
        assert!(item(&mut w, 1).selected);
        assert_eq!(item(&mut w, 1).label.content, "Explore");
    }

    // ---- Semantics ----------------------------------------------------------

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_state: &mut ()) -> NavigationRailView<()> {
            navigation_rail(one_section(), 1, |_s: &mut (), _i| {})
                .rail_type(NavigationRailType::Expanded)
        }
        let mut root: frust_core::RenderRoot<(), NavigationRailView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(AREA, &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let tablist = update
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::TabList)
            .expect("a TabList container node is contributed");
        assert!(
            tablist.1.children().len() >= 3,
            "every destination is forwarded beneath it"
        );

        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3);
        let selected = tabs
            .iter()
            .find(|(_, node)| node.label() == Some("Search"))
            .expect("the Search tab is present");
        assert_eq!(selected.1.is_selected(), Some(true));
        let unselected = tabs
            .iter()
            .find(|(_, node)| node.label() == Some("Home"))
            .expect("the Home tab is present");
        assert_eq!(unselected.1.is_selected(), Some(false));
    }
}
