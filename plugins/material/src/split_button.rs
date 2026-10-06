// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/split_buttons/` tree (17 files) is itself vendored from
// m3e_buttons (MIT, © 2026 Mudit Purohit) — `m3e_split_buttons.dart` plus its
// `components/` (widgets, style, content, segments, menu, gradient), `enums/`,
// `models/` and `styles/` parts, retrieved 2026-08-19.
// Porting decisions: upstream's third menu style
// (`M3ESplitButtonMenuStyle.native`, Flutter's own `showMenu`) is **descoped** —
// this framework hosts no platform menu to route to; the popup and bottom-sheet
// styles are both ported. The gradient decoration layers
// (`m3e_split_button_gradient.dart`), `M3EButtonSize.custom` per-instance
// measurement overrides, and the multi-select bottom sheet
// (`M3ESplitButtonSelectionMode.multiple`) are likewise out of scope — see the
// module docs' Not ported section.

//! The Material 3 Expressive **split button**: a leading action segment joined
//! to a trailing menu-trigger segment across a hairline gap, reading as one
//! connected shape.
//!
//! [`split_button`] keeps its original four-argument shape — label, the
//! controlled `open` flag, the leading `on_press` action, and the trailing
//! `on_open` menu callback — and every reference axis is a chained builder on
//! top: [`SplitButtonView::size`], [`SplitButtonView::variant`],
//! [`SplitButtonView::shape`], [`SplitButtonView::items`],
//! [`SplitButtonView::menu_style`].
//!
//! # Two segments, two focus targets
//!
//! The reference builds the halves as two independent `Focus` nodes inside one
//! `FocusTraversalGroup`, each with its own `InkWell`, its own
//! `canRequestFocus`/`skipTraversal` gate and its own key handler
//! (`m3e_split_button_segments.dart:100` and `:335`). This port matches that
//! structurally: each half is a real child widget in its own pod, so it is its
//! own **hit target**, its own **focus target** (a `Key` event routes down the
//! focus path to exactly one of them), its own **semantics node**, and
//! separately disable-able — [`SplitButtonView::enabled`] gates the whole
//! control (the reference's `IgnorePointer(ignoring: !enabled)`) while
//! [`SplitButtonView::trailing_enabled`] disables the menu trigger alone,
//! making real the `leadingEnabled`/`trailingEnabled` split the reference
//! already carries as two distinct variables fed from one prop
//! (`m3e_split_button_content.dart:212`-`:213`).
//!
//! **Honest limitation, the same one [`mod@crate::button`] records:** frust has
//! no keyboard focus *traversal* — nothing hands a button focus, and neither
//! segment claims focus on a press (a tap-shows-a-focus-ring button is the
//! wrong behavior on touch). The focus ring, the focus-widened gap and the
//! Enter/Space activation below are ported, gated on the authoritative focus
//! reads and unit-tested by seeding a pod's recorded focus path directly; they
//! become live the moment focus routing can reach a button.
//!
//! # Sizes
//!
//! Segment height / trailing width / leading paddings / icon, from
//! `M3ESplitButtonTheme`'s tables (`m3e_split_button_theme.dart:76`-`:210`),
//! with the label type role from `_LeadingContent`
//! (`m3e_split_button_widgets.dart:31`):
//!
//! | Size | Height | Trailing width | Left pad | Label right pad | Icon | Label role |
//! |---|---|---|---|---|---|---|
//! | [`SplitButtonSize::Xs`] | 32 | 48 | 12 | 10 | 20 | `label_small` |
//! | [`SplitButtonSize::Sm`] (default) | 40 | 48 | 16 | 12 | 24 | `label_medium` |
//! | [`SplitButtonSize::Md`] | 56 | 56 | 24 | 24 | 24 | `label_large` |
//! | [`SplitButtonSize::Lg`] | 96 | 96 | 48 | 48 | 32 | `title_medium` |
//! | [`SplitButtonSize::Xl`] | 136 | 136 | 64 | 64 | 40 | `title_large` |
//!
//! The trailing width column is the reference's own three-way resolution
//! (unselected = left inner pad + icon + right outer pad; selected = side pad ×
//! 2 + icon; circle = the segment height), floored at
//! [`SPLIT_MIN_TAP_TARGET`] — all three agree for every size, which
//! `the_three_trailing_width_formulas_agree` pins rather than assumes.
//!
//! # Variants
//!
//! Container / content roles are the plain button's
//! (`m3e_button_theme.dart:111`/`:127`, reached through
//! `_resolveColorsAndShapes`): [`SplitButtonVariant::Filled`] `primary` /
//! `on_primary`, [`SplitButtonVariant::Tonal`] `secondary_container` /
//! `on_secondary_container`, [`SplitButtonVariant::Elevated`]
//! `surface_container_low` / `primary` (plus a shadow),
//! [`SplitButtonVariant::Outlined`] transparent + `outline` hairline /
//! `primary`. There is no `Text` variant: the reference asserts it away
//! (`m3e_split_buttons.dart:76`).
//!
//! # Connected geometry and the spring clip
//!
//! Each segment resolves **four independent corner radii** and springs to
//! them, then clips its own content to the animating shape — the reference's
//! `M3ERadiusAndPaddingMotion` feeding a `Material(clipBehavior: antiAlias,
//! borderRadius: animatedRadius)` (`m3e_split_button_segments.dart:235`,
//! `:330`). The outer (world-facing) corners take the shape family's radius;
//! the two inner (adjacent) corners resolve per state:
//!
//! | State | Leading inner | Trailing inner | Trailing outer |
//! |---|---|---|---|
//! | resting | `inner` | `inner` | `outer` |
//! | hovered | `hovered` | `hovered` | `outer` |
//! | pressed | `pressed` | `pressed` | `outer` |
//! | menu open | — | `height / 2` | `height / 2` |
//!
//! `_leadingRadii`/`_trailingRadii` (`m3e_split_button_style.dart:166`/`:182`)
//! in precedence order — the open (`selected`) state outranks pressed, which
//! outranks hovered. The per-size inner column (4/4/4/8/12) lands exactly on
//! this crate's `extra_small`/`small`/`medium` shape tokens and therefore
//! resolves from the live [`frust::Theme`]; the hovered (8/12/12/20/20),
//! pressed (2/2/2/4/6) and square-outer (8/10/14/24/34) columns have no
//! whole-table token to resolve from and stay literal — the same
//! partial-coincidence rule [`mod@crate::toggle_button`] applies.
//!
//! The spring is [`SPLIT_SHAPE_SPRING`] — stiffness 380, damping ratio 0.55,
//! the `expressiveSpatialPress` preset every button in this family defaults to
//! (`m3e_base_button_state.dart:183`), i.e.
//! [`crate::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`]. It is a named constant
//! rather than a theme read because a press starts in the event pass, which
//! never threads a theme; `the_shape_spring_matches_expressive_spatial_press`
//! is the tripwire. [`ShapeMotion`] carries the retarget contract
//! [`mod@crate::button`]'s `RadiusPaddingMotion` documents — mount-frame snap,
//! a 0.1px dead band, mid-flight continuity from the value being painted right
//! now, and a 1.5× overshoot cap — re-pinned here directly, since that type is
//! private to a sibling module.
//!
//! # The chevron: a tween, not a spring
//!
//! Only the *clip* springs. The chevron itself is an `AnimatedRotation`
//! (`m3e_split_button_segments.dart:276`): [`CHEVRON_OPEN_TURNS`] (half a
//! turn) over [`CHEVRON_ROTATION_DURATION`] (120ms) on `Curve::EaseOut`,
//! transcribed as a duration-driven [`frust::AnimationController`] rather than
//! a fling. It also carries a per-size optical nudge
//! ([`SplitButtonTrailingAlignment::OpticalCenter`], the default) that drops to
//! zero while the menu is open, exactly as `_computeTrailingGeometry` does.
//!
//! # Menu styles
//!
//! [`SplitButtonMenuStyle`] is the ported half of the reference's three-way
//! `M3ESplitButtonMenuStyle`, and [`SplitButtonView::menu_route`] is the
//! selector: it hands back the presentation this button's style calls for,
//! built from the same [`SplitButtonView::items`] list.
//!
//! * [`SplitButtonMenuStyle::Popup`] → [`SplitButtonMenuRoute::Popup`], a
//!   [`crate::menu()`] under [`mod@crate::overlay::anchored`], anchored to the
//!   **trailing segment** at the reference's `bottomStart` + 4dp
//!   `popupOffset`. Like [`crate::button_group::ButtonGroupView::overflow_menu`],
//!   this is a second, app-mounted view (the top of the app's own
//!   [`frust::Stack`], or a transparent navigator page): the trigger paints
//!   inside this widget's own box and a leaf widget has no portal to reach out
//!   through. Wire [`SplitButtonView::menu_anchor`] so it has a rect to place
//!   against.
//! * [`SplitButtonMenuStyle::BottomSheet`] → [`SplitButtonMenuRoute::Sheet`], a
//!   [`crate::bottom_sheet`] over the same items, to hand to
//!   [`crate::show_bottom_sheet`] — which needs a
//!   [`frust::NavigatorController`] a leaf widget cannot reach, so presentation
//!   stays the app's exactly as it does for `button_group`'s sheet-style
//!   overflow. The staged back-press exit and the scrim are the sheet tier's,
//!   not this module's.
//! * `native` is **descoped** (plan decision): Flutter's `showMenu` is a
//!   platform-menu route with no analogue here, and the popup style already
//!   covers every case it served.
//!
//! Both routes render the item list through the one item-list surface this
//! catalog owns ([`crate::menu::menu_panel`]) rather than a second row
//! renderer — a porting decision, since the reference's sheet rows
//! (`m3e_split_button_bottom_sheet.dart:105`) are a near-duplicate of its own
//! menu rows.
//!
//! # Not ported
//!
//! `M3ESplitButtonDecoration`'s gradient layers (the plain button's
//! [`crate::ButtonDecoration`] seam is where that belongs), `M3EButtonSize.custom`,
//! the multi-select bottom sheet and its checkbox style, `menuBuilder`/
//! `m3eMenuBuilder` (an app composes [`crate::menu()`] itself), tooltips and
//! `onLongPress`. Each is either a seam this catalog already owns elsewhere or
//! a Flutter-plumbing concept with no frust analogue — the same mapping table
//! [`mod@crate::button`] records.
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section (Mudit Purohit / m3e_buttons) and its Module
//! Attribution Header Convention.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{
    FontWeight, LineHeight, TextContext, TextLayout, TextOverflow, TextStyle,
};
use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, CornerRadii,
    CursorIcon, ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any,
    build_child, erase_callback, rebuild_child, route_event, teardown_child, visit_children,
};
use frust::{AnimationController, Curve, FrameTime, IconData, SpringDesc, Theme};
use kurbo::{Affine, BezPath, RoundedRect, RoundedRectRadii, Shape, Vec2};

use crate::icons;
use crate::interaction::{
    DISABLED_CONTAINER_OPACITY, DISABLED_CONTENT_OPACITY, HapticSignal, InteractionState,
    MaterialHaptics,
};
use crate::menu::{MenuIcon, MenuNode, MenuSelection, menu, menu_panel, menu_selectable};
use crate::overlay::OverlayAnchor;
use crate::press::presses;
use crate::sheet::{BottomSheetView, bottom_sheet};

// ---- Theme defaults (m3e_split_button_theme.dart's constructor) ------------

/// The minimum tap target the control as a whole reserves, in logical px —
/// `M3ESplitButtonTheme.minTapTarget` (`:13`). A segment shorter than this
/// (the `xs`/`sm` rows) is centred inside it rather than stretched.
pub const SPLIT_MIN_TAP_TARGET: f64 = 48.0;

/// The gap between the two segments, in logical px — `innerGap` (`:14`).
pub const SPLIT_INNER_GAP: f64 = 2.0;

/// The gap an [`SplitButtonVariant::Elevated`] split button takes instead, in
/// logical px — `elevatedInnerGap` (`:15`), widened so two shadows do not
/// merge.
pub const SPLIT_ELEVATED_INNER_GAP: f64 = 4.0;

/// How far the chevron rotates when the menu opens, in turns —
/// `chevronOpenTurns` (`:16`).
pub const CHEVRON_OPEN_TURNS: f64 = 0.5;

/// The open trailing segment's corner radius as a percentage of its height —
/// `trailingInnerSelectedCornerPercent` (`:17`), i.e. a pill.
pub const TRAILING_SELECTED_CORNER_PERCENT: f64 = 50.0;

/// Stroke width of the focus ring, in logical px —
/// `M3EButtonConstants.kFocusRingWidth`.
const FOCUS_RING_WIDTH: f64 = 2.0;

/// Gap between a segment's edge and its focus ring, in logical px —
/// `M3EButtonConstants.kFocusRingGap`.
const FOCUS_RING_GAP: f64 = 2.0;

/// How far a focus ring extends past its segment, in logical px — the
/// `kFocusRingGap + kFocusRingWidth` sum `_computeSegmentGeometry` adds to the
/// inner gap while either half is focused (`m3e_split_button_content.dart:296`).
pub const SPLIT_FOCUS_RING_OUTSET: f64 = FOCUS_RING_GAP + FOCUS_RING_WIDTH;

/// The chevron rotation's duration — `AnimatedRotation(duration: 120ms)`
/// (`m3e_split_button_segments.dart:277`). A tween, not a spring; see the
/// [module docs](self).
pub const CHEVRON_ROTATION_DURATION: Duration = Duration::from_millis(120);

/// The chevron rotation's easing — `Curves.easeOut` (`:278`).
const CHEVRON_ROTATION_CURVE: Curve = Curve::EaseOut;

/// The segment shape (clip) spring: stiffness 380, damping ratio 0.55, mass 1 —
/// `expressiveSpatialPress`, i.e.
/// [`crate::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`]. See the
/// [module docs](self) for why it is a constant rather than a theme read.
pub const SPLIT_SHAPE_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};

/// Nominal period seeding a morph [`AnimationController`]'s clock; the motion is
/// spring-driven via `fling`, so this backs construction only (mirrors
/// `button::motion::PRESS_ANIM_PERIOD`).
const SHAPE_ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (progress-units/sec) for each shape leg's `fling` — the same
/// modest kick every spring in this crate uses.
const FLING_VELOCITY: f64 = 4.0;

/// The dead band (logical px, per corner) a new target must exceed before it
/// starts a new spring leg — the reference's `kSpringRetargetTolerance`
/// (`m3e_radius_and_padding_motion.dart:18`).
const RETARGET_TOLERANCE: f64 = 0.1;

/// How far past its target the spring's progress may read — the reference's
/// `rawFactor.clamp(0.0, 1.5)` (`:403`).
const OVERSHOOT_LIMIT: f64 = 1.5;

/// Stroke width of an [`SplitButtonVariant::Outlined`] hairline, in logical px.
const OUTLINE_WIDTH: f64 = 1.0;

/// Flattening tolerance for the `kurbo` rounded-rect paths this module strokes
/// (matches [`mod@crate::button_group`]'s `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// The ink every label run is *shaped* with; never painted — see
/// [`mod@crate::button`]'s `SHAPING_INK` for why (recolor-without-reshape).
const SHAPING_INK: Color = Color::BLACK;

/// The trailing segment's default accessible name, used when no
/// [`SplitButtonView::trailing_label`] is set.
const DEFAULT_TRAILING_LABEL: &str = "Show menu";

// ---- Size / shape / variant vocabulary -------------------------------------

/// One row of the M3 type scale, as `(size, line_height, letter_spacing,
/// weight)` — the unthemed fallback for a label role.
type TypeToken = (f32, f32, f32, FontWeight);

const LABEL_SMALL: TypeToken = (11.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_MEDIUM: TypeToken = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const TITLE_MEDIUM: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const TITLE_LARGE: TypeToken = (22.0, 28.0, 0.0, FontWeight::REGULAR);

/// The split button's size tier — the reference's `M3EButtonSize`
/// (`m3e_button_enums.dart:64`) as `M3ESplitButtonTheme` measures it. See the
/// [module docs](self)' Sizes table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitButtonSize {
    /// 32dp segments.
    Xs,
    /// 40dp segments (the default, `M3ESplitButton.size`'s own default).
    #[default]
    Sm,
    /// 56dp segments.
    Md,
    /// 96dp segments.
    Lg,
    /// 136dp segments.
    Xl,
}

impl SplitButtonSize {
    /// Every size, in table order — the axis a matrix test walks.
    pub const ALL: [SplitButtonSize; 5] = [
        SplitButtonSize::Xs,
        SplitButtonSize::Sm,
        SplitButtonSize::Md,
        SplitButtonSize::Lg,
        SplitButtonSize::Xl,
    ];

    /// A segment's height, in logical px — `_splitHeight` (`:76`).
    pub fn height(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 32.0,
            SplitButtonSize::Sm => 40.0,
            SplitButtonSize::Md => 56.0,
            SplitButtonSize::Lg => 96.0,
            SplitButtonSize::Xl => 136.0,
        }
    }

    /// The chevron's side length, in logical px — `_splitTrailingWidth`
    /// (`:84`), which `splitTrailingIconSize` reads as the icon size.
    fn trailing_icon_size(self) -> f64 {
        match self {
            SplitButtonSize::Xs | SplitButtonSize::Sm => 22.0,
            SplitButtonSize::Md => 26.0,
            SplitButtonSize::Lg => 38.0,
            SplitButtonSize::Xl => 50.0,
        }
    }

    /// The leading segment's icon side length, in logical px — `_splitIcon`
    /// (`:124`).
    fn icon_size(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 20.0,
            SplitButtonSize::Sm | SplitButtonSize::Md => 24.0,
            SplitButtonSize::Lg => 32.0,
            SplitButtonSize::Xl => 40.0,
        }
    }

    /// The leading segment's outer (world-facing) padding, in logical px —
    /// `_splitLeftOuterPadding` (`:164`).
    fn left_outer_padding(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 12.0,
            SplitButtonSize::Sm => 16.0,
            SplitButtonSize::Md => 24.0,
            SplitButtonSize::Lg => 48.0,
            SplitButtonSize::Xl => 64.0,
        }
    }

    /// The gap between the leading icon and its label, in logical px —
    /// `_splitGapIconToLabel` (`:172`).
    fn icon_to_label_gap(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 4.0,
            SplitButtonSize::Sm | SplitButtonSize::Md => 8.0,
            SplitButtonSize::Lg => 12.0,
            SplitButtonSize::Xl => 16.0,
        }
    }

    /// The leading segment's inner (gap-facing) padding, in logical px —
    /// `_splitLabelRightPadding` (`:180`).
    fn label_right_padding(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 10.0,
            SplitButtonSize::Sm => 12.0,
            SplitButtonSize::Md => 24.0,
            SplitButtonSize::Lg => 48.0,
            SplitButtonSize::Xl => 64.0,
        }
    }

    /// The trailing segment's inner (gap-facing) padding, in logical px —
    /// `_splitTrailingLeftInnerPadding` (`:188`).
    fn trailing_left_padding(self) -> f64 {
        match self {
            SplitButtonSize::Xs | SplitButtonSize::Sm => 13.0,
            SplitButtonSize::Md => 15.0,
            SplitButtonSize::Lg => 29.0,
            SplitButtonSize::Xl => 43.0,
        }
    }

    /// The trailing segment's outer (world-facing) padding, in logical px —
    /// `_splitRightOuterPadding` (`:196`).
    fn right_outer_padding(self) -> f64 {
        self.trailing_left_padding()
    }

    /// The trailing segment's side padding while the menu is open, in logical
    /// px — `_splitSidePaddingSelected` (`:204`).
    fn side_padding_selected(self) -> f64 {
        self.trailing_left_padding()
    }

    /// The closed trailing segment's laid-out width, in logical px — see
    /// [`Self::trailing_width_when`], of which this is the resting case. All
    /// three of the reference's formulas agree for every size, so the trailing
    /// half never changes width when the menu opens.
    pub fn trailing_width(self) -> f64 {
        self.trailing_width_when(false, false)
    }

    /// The reference's three-way trailing-width resolution
    /// (`_trailingPadsAndWidth`/`_computeTrailingGeometry`): the segment height
    /// while collapsed to a circle, the selected width while the menu is open,
    /// the unselected width otherwise — floored at [`SPLIT_MIN_TAP_TARGET`]
    /// (`effectiveWidth`, `m3e_split_button_segments.dart:219`).
    fn trailing_width_when(self, open: bool, circle: bool) -> f64 {
        let base = if circle {
            self.height()
        } else if open {
            self.trailing_width_selected()
        } else {
            self.trailing_width_unselected()
        };
        base.max(SPLIT_MIN_TAP_TARGET)
    }

    /// `trailingWidthUnselected`: inner pad + chevron + outer pad.
    fn trailing_width_unselected(self) -> f64 {
        self.trailing_left_padding() + self.trailing_icon_size() + self.right_outer_padding()
    }

    /// `trailingWidthSelected`: side pad × 2 + chevron.
    fn trailing_width_selected(self) -> f64 {
        self.side_padding_selected() * 2.0 + self.trailing_icon_size()
    }

    /// Whether an open, round trailing segment collapses to a circle —
    /// `allowCircle` (`_computeTrailingGeometry`, `md`/`lg`/`xl` only).
    fn allows_circle_trailing(self) -> bool {
        matches!(
            self,
            SplitButtonSize::Md | SplitButtonSize::Lg | SplitButtonSize::Xl
        )
    }

    /// The chevron's optical-centre nudge along x, in logical px —
    /// `_splitMenuIconOffset` (`:116`). Negative: the glyph sits slightly
    /// leading-of-centre while the menu is closed.
    fn menu_icon_offset(self) -> f64 {
        match self {
            SplitButtonSize::Xs | SplitButtonSize::Sm => -1.0,
            SplitButtonSize::Md => -2.0,
            SplitButtonSize::Lg => -3.0,
            SplitButtonSize::Xl => -6.0,
        }
    }

    /// The resting inner-corner radius — `_splitInnerCornerRadius` (`:92`:
    /// 4/4/4/8/12). The table lands exactly on the `extra_small`/`small`/
    /// `medium` shape tokens, so a themed pass resolves it from the live
    /// [`frust::Theme`] and the literals are only the unthemed fallback.
    fn inner_radius(self, theme: Option<&Theme>) -> f64 {
        match theme {
            Some(theme) => match self {
                SplitButtonSize::Xs | SplitButtonSize::Sm | SplitButtonSize::Md => {
                    theme.shape.extra_small
                }
                SplitButtonSize::Lg => theme.shape.small,
                SplitButtonSize::Xl => theme.shape.medium,
            },
            None => match self {
                SplitButtonSize::Xs | SplitButtonSize::Sm | SplitButtonSize::Md => 4.0,
                SplitButtonSize::Lg => 8.0,
                SplitButtonSize::Xl => 12.0,
            },
        }
    }

    /// The hovered inner-corner radius — `_splitHoveredInnerCornerRadius`
    /// (`:100`: 8/12/12/20/20). No whole-table shape-scale coincidence, so this
    /// stays literal (the [module docs](self)' partial-coincidence rule).
    fn hovered_inner_radius(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 8.0,
            SplitButtonSize::Sm | SplitButtonSize::Md => 12.0,
            SplitButtonSize::Lg | SplitButtonSize::Xl => 20.0,
        }
    }

    /// The pressed corner radius — `_splitPressedRadius` (`:148`:
    /// 2/2/2/4/6). Literal: this crate's shape scale has no 2dp rung.
    fn pressed_radius(self) -> f64 {
        match self {
            SplitButtonSize::Xs | SplitButtonSize::Sm | SplitButtonSize::Md => 2.0,
            SplitButtonSize::Lg => 4.0,
            SplitButtonSize::Xl => 6.0,
        }
    }

    /// The outer corner radius under [`SplitButtonShape::Square`] —
    /// `_splitOuterRadiusSquare` (`:140`: 8/10/14/24/34). Literal, for the same
    /// partial-coincidence reason as the hovered column.
    fn square_outer_radius(self) -> f64 {
        match self {
            SplitButtonSize::Xs => 8.0,
            SplitButtonSize::Sm => 10.0,
            SplitButtonSize::Md => 14.0,
            SplitButtonSize::Lg => 24.0,
            SplitButtonSize::Xl => 34.0,
        }
    }

    /// The label's type role — `_LeadingContent`'s per-size switch
    /// (`m3e_split_button_widgets.dart:31`). Themed from [`frust::Theme`]'s
    /// type scale; unthemed from the M3 token literals above. The returned
    /// style always carries [`SHAPING_INK`].
    fn label_style(self, theme: Option<&Theme>) -> TextStyle {
        let mut style = match theme {
            Some(theme) => match self {
                SplitButtonSize::Xs => theme.type_scale.label_small.clone(),
                SplitButtonSize::Sm => theme.type_scale.label_medium.clone(),
                SplitButtonSize::Md => theme.type_scale.label_large.clone(),
                SplitButtonSize::Lg => theme.type_scale.title_medium.clone(),
                SplitButtonSize::Xl => theme.type_scale.title_large.clone(),
            },
            None => {
                let (size, line_height, letter_spacing, weight) = match self {
                    SplitButtonSize::Xs => LABEL_SMALL,
                    SplitButtonSize::Sm => LABEL_MEDIUM,
                    SplitButtonSize::Md => LABEL_LARGE,
                    SplitButtonSize::Lg => TITLE_MEDIUM,
                    SplitButtonSize::Xl => TITLE_LARGE,
                };
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
}

/// The split button's corner-radius family — the reference's `M3EButtonShape`
/// (`m3e_button_enums.dart:45`) as `_computeRadii` consumes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitButtonShape {
    /// A pill: the segment's `height / 2` (the default).
    #[default]
    Round,
    /// The per-size token radius ([`SplitButtonSize::square_outer_radius`]).
    Square,
}

/// The split button's container treatment — the reference's `M3EButtonStyle`
/// minus `text`, which `M3ESplitButton` asserts away
/// (`m3e_split_buttons.dart:76`). See the [module docs](self)' Variants
/// section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitButtonVariant {
    /// Solid `primary` container, highest emphasis (the default).
    #[default]
    Filled,
    /// `secondary_container` container, medium emphasis.
    Tonal,
    /// `surface_container_low` container that carries a shadow.
    Elevated,
    /// Transparent container with an `outline` hairline.
    Outlined,
}

impl SplitButtonVariant {
    /// Every variant, in table order — the axis a matrix test walks.
    pub const ALL: [SplitButtonVariant; 4] = [
        SplitButtonVariant::Filled,
        SplitButtonVariant::Tonal,
        SplitButtonVariant::Elevated,
        SplitButtonVariant::Outlined,
    ];
}

/// How the trailing chevron is centred — the reference's
/// `M3ESplitButtonTrailingAlignment`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitButtonTrailingAlignment {
    /// Nudge the glyph by [`SplitButtonSize::menu_icon_offset`] while the menu
    /// is closed (the default).
    #[default]
    OpticalCenter,
    /// Sit on the geometric centre in every state.
    GeometricCenter,
}

/// Which presentation the trailing trigger's menu takes — the ported half of
/// the reference's `M3ESplitButtonMenuStyle`. See the [module docs](self)'
/// Menu styles section (the third, `native`, is descoped).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplitButtonMenuStyle {
    /// An anchored popup menu under the trailing segment (the default).
    #[default]
    Popup,
    /// A modal bottom sheet listing the same items.
    BottomSheet,
}

// ---- Items ------------------------------------------------------------------

/// One selectable entry in a split button's menu — the reference's
/// `M3ESplitButtonItem` (`models/m3e_split_button_item.dart`).
///
/// `value` is what a selection reports back; it defaults to the label, which is
/// what `_splitItemToMenuNode`'s `item.child.toString()` fallback amounts to
/// for the text-child case this port supports.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitButtonItem {
    label: String,
    value: Option<String>,
    enabled: bool,
    leading: Option<MenuIcon>,
}

/// Create a menu item labelled `label`.
pub fn split_button_item(label: impl Into<String>) -> SplitButtonItem {
    SplitButtonItem {
        label: label.into(),
        value: None,
        enabled: true,
        leading: None,
    }
}

impl SplitButtonItem {
    /// Set the value a selection of this row reports (defaults to the label).
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Whether the row can be activated (`M3ESplitButtonItem.enabled`).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Attach a leading glyph (`_splitItemToMenuNode`'s `IconData` child arm).
    pub fn leading(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.leading = Some(icon.into());
        self
    }

    /// The row's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The value this row reports — its own, or its label.
    pub fn resolved_value(&self) -> &str {
        self.value.as_deref().unwrap_or(&self.label)
    }

    /// This item as a menu node — the port of `_splitItemToMenuNode`
    /// (`m3e_split_button_menu.dart:134`), which maps every item onto an
    /// `M3EMenuSelectable`.
    fn to_menu_node(&self) -> MenuNode {
        let mut node = menu_selectable(self.label.clone(), self.resolved_value().to_string())
            .enabled(self.enabled);
        if let Some(icon) = self.leading {
            node = node.leading(icon);
        }
        node.into()
    }
}

/// The presentation a split button's [`SplitButtonMenuStyle`] calls for, as
/// [`SplitButtonView::menu_route`] resolves it. See the [module docs](self)'
/// Menu styles section for how an app mounts each.
pub enum SplitButtonMenuRoute<State: 'static> {
    /// [`SplitButtonMenuStyle::Popup`]: an anchored menu to mount as the top of
    /// the app's own [`frust::Stack`] (or a transparent navigator page).
    Popup(AnyView<State>),
    /// [`SplitButtonMenuStyle::BottomSheet`]: sheet content to hand to
    /// [`crate::show_bottom_sheet`].
    Sheet(BottomSheetView<State>),
}

// ---- Colors -----------------------------------------------------------------

/// Unthemed-fallback `primary`.
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed-fallback `on_primary`.
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback `secondary_container`.
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `on_secondary_container`.
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `surface_container_low`.
const SURFACE_CONTAINER_LOW: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback `outline`.
const OUTLINE: Color = Color::from_rgb8(0x79, 0x74, 0x7E);
/// Unthemed-fallback `on_surface` — the disabled roles' base.
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback shadow color (opaque black at `crate::tokens::elevation()`'s
/// 0.3 alpha).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The resolved inks one segment paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SegmentColors {
    /// The container fill (transparent for an enabled `Outlined` segment).
    container: Color,
    /// The label/icon ink, and the state layer's tint.
    content: Color,
    /// The hairline, when the variant draws one.
    outline: Option<Color>,
    /// The focus ring's stroke (`primary`, `m3e_focus_ring.dart`).
    focus_ring: Color,
}

/// `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Resolve one segment's inks — `_resolveColorsAndShapes`/`_resolveDisabledColors`
/// (`m3e_split_button_style.dart:102`/`:129`) over the plain button's own
/// variant table. See the [module docs](self)' Variants section.
fn resolve_colors(
    theme: Option<&Theme>,
    variant: SplitButtonVariant,
    enabled: bool,
) -> SegmentColors {
    let (
        primary,
        on_primary,
        secondary_container,
        on_secondary_container,
        container_low,
        outline,
        on_surface,
    ) = match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.primary,
                s.on_primary,
                s.secondary_container,
                s.on_secondary_container,
                s.surface_container_low,
                s.outline,
                s.on_surface,
            )
        }
        None => (
            PRIMARY,
            ON_PRIMARY,
            SECONDARY_CONTAINER,
            ON_SECONDARY_CONTAINER,
            SURFACE_CONTAINER_LOW,
            OUTLINE,
            ON_SURFACE,
        ),
    };
    let transparent = variant == SplitButtonVariant::Outlined;

    if !enabled {
        return SegmentColors {
            container: if transparent {
                Color::TRANSPARENT
            } else {
                with_alpha(on_surface, DISABLED_CONTAINER_OPACITY)
            },
            content: with_alpha(on_surface, DISABLED_CONTENT_OPACITY),
            outline: transparent.then(|| with_alpha(on_surface, DISABLED_CONTAINER_OPACITY)),
            focus_ring: primary,
        };
    }

    let (container, content) = match variant {
        SplitButtonVariant::Filled => (primary, on_primary),
        SplitButtonVariant::Tonal => (secondary_container, on_secondary_container),
        SplitButtonVariant::Elevated => (container_low, primary),
        SplitButtonVariant::Outlined => (Color::TRANSPARENT, primary),
    };
    SegmentColors {
        container,
        content,
        outline: transparent.then_some(outline),
        focus_ring: primary,
    }
}

/// A segment's container elevation in dp — `_segmentElevation`
/// (`m3e_split_button_style.dart:195`) reading the plain button's own
/// elevation table.
fn elevation_dp(variant: SplitButtonVariant, enabled: bool, pressed: bool, hovered: bool) -> f64 {
    if !enabled {
        return 0.0;
    }
    match variant {
        SplitButtonVariant::Elevated => {
            if pressed {
                0.0
            } else if hovered {
                3.0
            } else {
                1.0
            }
        }
        SplitButtonVariant::Filled | SplitButtonVariant::Tonal => {
            if !pressed && hovered {
                1.0
            } else {
                0.0
            }
        }
        SplitButtonVariant::Outlined => 0.0,
    }
}

/// The `(blur_std_dev, y_offset, color)` shadow for an elevation of `dp`, or
/// `None` for a flat segment — the same M3 elevation-level resolution
/// [`mod@crate::toggle_button`] performs (duplicated for the same
/// private-module reason).
fn resolve_shadow(theme: Option<&Theme>, dp: f64) -> Option<(f64, f64, Color)> {
    if dp <= 0.0 {
        return None;
    }
    match theme {
        Some(theme) => {
            let level = if dp >= theme.elevation.level2.dp {
                theme.elevation.level2
            } else {
                theme.elevation.level1
            };
            let shadow = level.shadow(theme.brightness);
            Some((
                shadow.blur_std_dev,
                shadow.y_offset,
                with_alpha(theme.scheme().shadow, shadow.color_alpha),
            ))
        }
        None => Some((dp, dp / 2.0 + 1.0, FALLBACK_SHADOW_COLOR)),
    }
}

// ---- The per-corner shape (clip) spring -------------------------------------

/// Linear interpolation between `from` and `to` at `t` (unclamped — the
/// spring's overshoot rides past `t = 1`).
fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

/// Whether every corner of `a` is within [`RETARGET_TOLERANCE`] of `b`'s.
fn corners_match(a: CornerRadii, b: CornerRadii) -> bool {
    (a.top_left - b.top_left).abs() <= RETARGET_TOLERANCE
        && (a.top_right - b.top_right).abs() <= RETARGET_TOLERANCE
        && (a.bottom_right - b.bottom_right).abs() <= RETARGET_TOLERANCE
        && (a.bottom_left - b.bottom_left).abs() <= RETARGET_TOLERANCE
}

/// Per-corner interpolation of `from` toward `to` at `t`, each corner floored at
/// `0` (the reference's `v < 0 ? 0.0 : v` corner guard).
fn lerp_corners(from: CornerRadii, to: CornerRadii, t: f64) -> CornerRadii {
    CornerRadii::new(
        lerp(from.top_left, to.top_left, t).max(0.0),
        lerp(from.top_right, to.top_right, t).max(0.0),
        lerp(from.bottom_right, to.bottom_right, t).max(0.0),
        lerp(from.bottom_left, to.bottom_left, t).max(0.0),
    )
}

/// A segment's four-corner shape spring — the radius channel of the reference's
/// `M3ERadiusAndPaddingMotion`. The padding channel is not carried: a split
/// button's segment paddings are pure functions of its size and never retarget
/// (`internalLeft`..`internalBottom` are all passed `0`,
/// `m3e_split_button_segments.dart:32`).
///
/// See the [module docs](self) for the retarget/continuity contract this shares
/// with [`mod@crate::button`]'s `RadiusPaddingMotion`.
#[derive(Clone, Copy, Debug)]
struct ShapeMotion {
    from_radii: CornerRadii,
    to_radii: CornerRadii,
    anim: AnimationController,
    /// Whether a first target has been seeded — the mount frame snaps rather
    /// than springs.
    seeded: bool,
}

impl ShapeMotion {
    /// An unseeded morph resting at every corner `0`; the first
    /// [`Self::retarget`] snaps instead of springing.
    fn new() -> Self {
        Self {
            from_radii: CornerRadii::default(),
            to_radii: CornerRadii::default(),
            anim: AnimationController::new(SHAPE_ANIM_PERIOD),
            seeded: false,
        }
    }

    /// Aim the morph at `radii`, returning whether this started a new leg.
    fn retarget(&mut self, radii: CornerRadii) -> bool {
        if !self.seeded {
            self.snap_to(radii);
            return false;
        }
        if corners_match(self.to_radii, radii) {
            return false;
        }
        self.from_radii = self.radii();
        self.to_radii = radii;
        self.anim = AnimationController::new(SHAPE_ANIM_PERIOD);
        self.anim.fling(FLING_VELOCITY, SPLIT_SHAPE_SPRING);
        true
    }

    /// Pin the morph to `radii` with no motion at all, marking it seeded.
    fn snap_to(&mut self, radii: CornerRadii) {
        self.from_radii = radii;
        self.to_radii = radii;
        self.anim = AnimationController::new(SHAPE_ANIM_PERIOD);
        self.seeded = true;
    }

    /// Advance the spring to frame time `now`, reporting whether it is still
    /// animating.
    fn advance(&mut self, now: FrameTime) -> bool {
        self.anim.advance(now)
    }

    /// Whether a first target has been seeded.
    fn is_seeded(&self) -> bool {
        self.seeded
    }

    /// The progress factor this frame paints at.
    fn factor(&self) -> f64 {
        let raw = self.anim.value();
        if raw.is_finite() {
            raw.clamp(0.0, OVERSHOOT_LIMIT)
        } else {
            0.0
        }
    }

    /// The per-corner radii to paint (and clip to) this frame.
    fn radii(&self) -> CornerRadii {
        lerp_corners(self.from_radii, self.to_radii, self.factor())
    }

    /// The radii the morph is currently springing toward.
    #[cfg(test)]
    fn target_radii(&self) -> CornerRadii {
        self.to_radii
    }
}

// ---- Geometry helpers -------------------------------------------------------

/// Whether local point `pos` is inside a widget of `size`.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The leading segment's corner radii — `_leadingRadii`
/// (`m3e_split_button_style.dart:166`): outer on the world-facing (left) side,
/// the state-resolved inner radius on the gap-facing (right) one.
///
/// This framework has no `Directionality`/RTL layout, so — like
/// [`mod@crate::button_group`]'s `member_radii` — start/end map onto left/right
/// unconditionally, with no mirrored-in-RTL case to port.
fn leading_radii(outer: f64, inner: f64) -> CornerRadii {
    CornerRadii::new(outer, inner, inner, outer)
}

/// The trailing segment's corner radii — `_trailingRadii` (`:182`): the
/// state-resolved inner radius on the gap-facing (left) side, outer on the
/// world-facing (right) one.
fn trailing_radii(outer: f64, inner: f64) -> CornerRadii {
    CornerRadii::new(inner, outer, outer, inner)
}

/// The rounded rect to *stroke* for a `width`-wide hairline lying fully inside
/// a container of `size` with per-corner `radii` — a stroke is centred on its
/// path, so both the rect and every radius pull in by half the width.
fn inset_stroke_rrect(size: Size, radii: CornerRadii, width: f64) -> RoundedRect {
    let half = width / 2.0;
    RoundedRect::from_rect(
        Rect::new(half, half, size.width - half, size.height - half),
        RoundedRectRadii::new(
            (radii.top_left - half).max(0.0),
            (radii.top_right - half).max(0.0),
            (radii.bottom_right - half).max(0.0),
            (radii.bottom_left - half).max(0.0),
        ),
    )
}

/// The rounded rect to *stroke* for the focus ring around a segment of `size`
/// with per-corner `radii` — [`FOCUS_RING_GAP`] outside the container, with the
/// stroke itself centred on that path, so every radius grows by the same
/// outset (`m3e_focus_ring.dart`).
fn focus_ring_rrect(size: Size, radii: CornerRadii) -> RoundedRect {
    let out = FOCUS_RING_GAP + FOCUS_RING_WIDTH / 2.0;
    RoundedRect::from_rect(
        Rect::new(-out, -out, size.width + out, size.height + out),
        RoundedRectRadii::new(
            radii.top_left + out,
            radii.top_right + out,
            radii.bottom_right + out,
            radii.bottom_left + out,
        ),
    )
}

// ---- The label run ----------------------------------------------------------

/// The leading segment's lazily-shaped label run, re-brushed at paint time — a
/// sibling of `button::core::LabelRun` (private to that module), the same shape
/// [`mod@crate::toggle_button`] carries.
struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    shaped_for: Option<(TextStyle, Option<f64>)>,
    natural: Option<(TextStyle, f64)>,
}

impl LabelRun {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_for: None,
            natural: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
            self.shaped_for = None;
            self.natural = None;
        }
    }

    fn content(&self) -> &str {
        &self.content
    }

    fn natural_width(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> f64 {
        if let Some((cached_style, width)) = &self.natural
            && cached_style == style
        {
            return *width;
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let width = text_ctx.layout(&self.content, style, None).size().width;
        self.natural = Some((style.clone(), width));
        width
    }

    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f64>) -> Size {
        let key = (style.clone(), max_width);
        if let Some(cached) = &self.layout
            && self.shaped_for.as_ref() == Some(&key)
        {
            return cached.size();
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

// ---- One segment ------------------------------------------------------------

/// Which half a segment is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmentKind {
    /// The action half: label (+ optional icon), fires `on_press`.
    Leading,
    /// The menu-trigger half: the chevron, fires `on_open`.
    Trailing,
}

/// The declarative half of one segment. Private: a split button's segments are
/// its own composition, never something an app builds directly.
struct SegmentView<State: 'static> {
    kind: SegmentKind,
    size: SplitButtonSize,
    shape: SplitButtonShape,
    variant: SplitButtonVariant,
    enabled: bool,
    open: bool,
    trailing_alignment: SplitButtonTrailingAlignment,
    label: String,
    icon: Option<AnyView<State>>,
    haptic: HapticSignal,
    /// The trailing segment's anchor cell, written on every paint — the same
    /// one-line window-rect capture [`crate::overlay::overlay_anchor`] performs,
    /// inlined so the segment stays one pod (and therefore one focus target).
    anchor: Option<OverlayAnchor>,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// The retained half of one segment.
struct SegmentWidget {
    kind: SegmentKind,
    size: SplitButtonSize,
    shape: SplitButtonShape,
    variant: SplitButtonVariant,
    enabled: bool,
    open: bool,
    trailing_alignment: SplitButtonTrailingAlignment,
    label: LabelRun,
    label_origin: Point,
    icon: Option<ChildPod>,
    haptic: HapticSignal,
    anchor: Option<OverlayAnchor>,
    /// Hover/focus/pressed tracking feeding the state layer and the shape
    /// spring's targets.
    state: InteractionState,
    /// Whether this segment holds the pointer capture a `Down` took.
    captured: bool,
    /// The four-corner clip spring.
    motion: ShapeMotion,
    /// The chevron's parsed path + design box (trailing only).
    chevron: Option<(BezPath, f64)>,
    /// The chevron rotation tween (trailing only, inert on the leading half).
    chevron_anim: AnimationController,
    on_press: ErasedCallback,
}

impl SegmentWidget {
    /// Whether an open, round trailing segment collapses to a circle —
    /// `circleTrailing` (`_computeTrailingGeometry`).
    ///
    /// Under the ported subset this is *equivalent* to the ordinary open
    /// branch: `trailingSelectedRadius` is `height / 2`, which is exactly what
    /// [`SplitButtonShape::Round`]'s outer radius already resolves to, and the
    /// circle branch only fires for round shapes. It is transcribed anyway, and
    /// `the_circle_trailing_branch_coincides_with_the_open_branch` pins the
    /// coincidence rather than leaving it as folklore.
    fn circle_trailing(&self) -> bool {
        self.kind == SegmentKind::Trailing
            && self.open
            && self.shape == SplitButtonShape::Round
            && self.size.allows_circle_trailing()
    }

    /// This frame's target corner radii — `_leadingRadii`/`_trailingRadii` in
    /// their documented precedence. See the [module docs](self)' Connected
    /// geometry table.
    fn target_radii(&self, theme: Option<&Theme>, height: f64) -> CornerRadii {
        let outer = match self.shape {
            SplitButtonShape::Round => height / 2.0,
            SplitButtonShape::Square => self.size.square_outer_radius(),
        };
        let selected = height * (TRAILING_SELECTED_CORNER_PERCENT / 100.0);
        let pressed = self.enabled && self.state.pressed;
        // The reference suppresses the trailing hover shape while the menu is
        // open (`trailingHovered`, `m3e_split_button_content.dart:304`).
        let hovered = self.enabled && self.state.hovered && !(self.open && self.is_trailing());

        match self.kind {
            SegmentKind::Leading => {
                let inner = if pressed {
                    self.size.pressed_radius()
                } else if hovered {
                    self.size.hovered_inner_radius()
                } else {
                    self.size.inner_radius(theme)
                };
                leading_radii(outer, inner)
            }
            SegmentKind::Trailing => {
                if self.circle_trailing() {
                    return CornerRadii::uniform(selected);
                }
                let outer = if self.open { selected } else { outer };
                let inner = if self.open {
                    selected
                } else if pressed {
                    self.size.pressed_radius()
                } else if hovered {
                    self.size.hovered_inner_radius()
                } else {
                    self.size.inner_radius(theme)
                };
                trailing_radii(outer, inner)
            }
        }
    }

    fn is_trailing(&self) -> bool {
        self.kind == SegmentKind::Trailing
    }

    /// The chevron's transformed path for this frame, or `None` on the leading
    /// half — `_buildTrailingChevron`'s `AnimatedRotation` over a
    /// `Transform.translate`d icon.
    fn chevron_path(&self, size: Size) -> Option<BezPath> {
        let (path, design) = self.chevron.as_ref()?;
        if *design <= 0.0 {
            return None;
        }
        let dx = match self.trailing_alignment {
            SplitButtonTrailingAlignment::OpticalCenter
                if !self.circle_trailing() && !self.open =>
            {
                self.size.menu_icon_offset()
            }
            _ => 0.0,
        };
        let center = Point::new(size.width / 2.0 + dx, size.height / 2.0);
        let angle = self.chevron_anim.value() * CHEVRON_OPEN_TURNS * std::f64::consts::TAU;
        let scale = self.size.trailing_icon_size() / design;
        let transform = Affine::translate(Vec2::new(center.x, center.y))
            * Affine::rotate(angle)
            * Affine::scale(scale)
            * Affine::translate(Vec2::new(-design / 2.0, -design / 2.0));
        Some(transform * path.clone())
    }

    /// Whether `key` is one of the reference's activation keys —
    /// `_leadingKeyEvent`/`_trailingKeyEvent` (Enter, Space, NumpadEnter; this
    /// framework's `NamedKey` has no separate numpad Enter).
    fn activates(key: &Key) -> bool {
        match key {
            Key::Named(NamedKey::Enter) => true,
            Key::Character(text) => text == " ",
            Key::Named(_) => false,
        }
    }

    /// Fire the segment's action plus its haptic, the up-inside/key-activate
    /// edge both entry points share.
    fn activate(&mut self, ctx: &mut EventCtx) {
        if self.haptic != HapticSignal::None {
            MaterialHaptics::fire(self.haptic);
        }
        (self.on_press)(ctx);
    }
}

impl<State: 'static> View<State> for SegmentView<State> {
    type Element = SegmentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SegmentWidget {
        let mut chevron_anim =
            AnimationController::new(CHEVRON_ROTATION_DURATION).with_curve(CHEVRON_ROTATION_CURVE);
        if self.kind == SegmentKind::Trailing && self.open {
            // Mounted already-open: rest at the rotated pose rather than
            // animating into it on the first frame.
            chevron_anim.animate_to(1.0);
            chevron_anim.stop();
        }
        SegmentWidget {
            kind: self.kind,
            size: self.size,
            shape: self.shape,
            variant: self.variant,
            enabled: self.enabled,
            open: self.open,
            trailing_alignment: self.trailing_alignment,
            // Both kinds carry their resolved label: the leading half paints
            // it, the trailing half only exposes it through semantics (the
            // menu-trigger's accessible name) — [`SplitButtonView::trailing_view`]
            // already resolved `trailing_label` against
            // [`DEFAULT_TRAILING_LABEL`] before this view was built.
            label: LabelRun::new(self.label.clone()),
            label_origin: Point::ZERO,
            icon: self.icon.as_ref().map(|icon| build_child(icon, ctx)),
            haptic: self.haptic,
            anchor: self.anchor.clone(),
            state: InteractionState::new(),
            captured: false,
            motion: ShapeMotion::new(),
            chevron: (self.kind == SegmentKind::Trailing)
                .then(|| IconData::from(icons::KEYBOARD_ARROW_DOWN).resolve()),
            chevron_anim,
            on_press: erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SegmentWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = erase_callback(&self.on_press);
        element.haptic = self.haptic;
        element.anchor = self.anchor.clone();
        let mut flags = ChangeFlags::NONE;

        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.shape != self.shape {
            element.shape = self.shape;
            flags |= ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.trailing_alignment != self.trailing_alignment {
            element.trailing_alignment = self.trailing_alignment;
            flags |= ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // A segment disabled mid-press keeps neither the press nor the
                // capture.
                element.state.set_pressed(false);
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.open != self.open {
            element.open = self.open;
            if element.kind == SegmentKind::Trailing {
                element
                    .chevron_anim
                    .animate_to(if self.open { 1.0 } else { 0.0 });
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label.set_content(&self.label);
            flags |= if self.kind == SegmentKind::Leading {
                // The leading half paints its label, so a change reflows it.
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            } else {
                // The trailing half's label is accessibility-only (the menu
                // trigger's accessible name) — no child to reconcile, the
                // same shape `fab.rs`'s icon-only-FAB label rebuild takes.
                ChangeFlags::PAINT
            };
        }

        match (prev.icon.as_ref(), self.icon.as_ref()) {
            (None, None) => {}
            (Some(p), Some(n)) => {
                let pod = element.icon.as_mut().expect("icon pod present");
                flags |= rebuild_child(p, n, pod, ctx);
            }
            (None, Some(n)) => {
                element.icon = Some(build_child(n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None) => {
                let mut pod = element.icon.take().expect("icon pod present");
                teardown_child(p, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut SegmentWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(icon), Some(pod)) = (self.icon.as_ref(), element.icon.as_mut()) {
            teardown_child(icon, pod, ctx);
        }
    }
}

impl Widget for SegmentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let height = self.size.height();
        if !self.motion.is_seeded() {
            let theme = Theme::from_layout_ctx(ctx);
            self.motion.snap_to(self.target_radii(theme, height));
        }

        if self.kind == SegmentKind::Trailing {
            let width = self
                .size
                .trailing_width_when(self.open, self.circle_trailing());
            return bc.constrain(Size::new(width, height));
        }

        let left = self.size.left_outer_padding();
        let right = self.size.label_right_padding();
        let icon_size = self.icon.as_mut().map(|pod| {
            let side = self.size.icon_size();
            pod.layout_child(ctx, &BoxConstraints::tight(Size::new(side, side)))
        });
        let icon_width = icon_size.map_or(0.0, |s| s.width);
        let has_label = !self.label.content().is_empty();
        let gap = if icon_size.is_some() && has_label {
            self.size.icon_to_label_gap()
        } else {
            0.0
        };

        let style = self.size.label_style(Theme::from_layout_ctx(ctx));
        let available = if bc.max().width.is_finite() {
            (bc.max().width - left - right - icon_width - gap).max(0.0)
        } else {
            f64::INFINITY
        };
        let natural = self.label.natural_width(ctx, &style);
        let fit_to = (natural > available)
            .then_some(available)
            .filter(|w| w.is_finite());
        let label_size = self.label.shape(ctx, &style, fit_to);

        let width = left + icon_width + gap + label_size.width + right;
        let size = bc.constrain(Size::new(width, height));

        if let Some(pod) = self.icon.as_mut() {
            let icon_height = icon_size.map_or(0.0, |s| s.height);
            pod.set_origin(Point::new(left, (size.height - icon_height) / 2.0));
        }
        self.label_origin = Point::new(
            left + icon_width + gap,
            (size.height - label_size.height) / 2.0,
        );
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();

        // The one-line window-rect capture the anchored host's own trigger
        // wrapper performs (`crate::overlay::anchored`'s `OverlayAnchorWidget`).
        if let Some(anchor) = &self.anchor {
            anchor.set(Rect::from_origin_size(origin, size));
        }

        // Authoritative hover/focus reads, self-correcting the latched flags
        // (`docs/CODE_STANDARDS.md`'s Interaction Semantics); both inert while
        // disabled, mirroring the reference's `enabled` guards.
        self.state.set_hovered(self.enabled && ctx.is_hovered());
        self.state.set_focused(self.enabled && ctx.has_focus());

        let (colors, shadow, radii_target) = {
            let theme = Theme::from_paint_ctx(ctx);
            let dp = elevation_dp(
                self.variant,
                self.enabled,
                self.state.pressed,
                self.state.hovered,
            );
            (
                resolve_colors(theme, self.variant, self.enabled),
                resolve_shadow(theme, dp),
                self.target_radii(theme, size.height),
            )
        };

        if self.motion.retarget(radii_target) {
            ctx.request_frame();
        }
        if self.motion.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        if self.chevron_anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let radii = self.motion.radii();

        if let Some((blur, y_offset, shadow_color)) = shadow {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y_offset),
                size,
                radii.largest(),
                blur,
                shadow_color,
            );
        }
        if colors.container.components[3] > 0.0 {
            scene.fill_rounded_rect_radii(origin, size, radii, colors.container);
        }

        // The reference's `Material(clipBehavior: Clip.antiAlias, borderRadius:
        // animatedRadius)`: everything inside the segment is masked by the
        // *animating* shape, which is what makes the morph read as a clip
        // rather than a redraw.
        scene.push_clip_rounded_radii(origin, size, radii);
        if self.enabled {
            let opacity = self.state.resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect_radii(
                    origin,
                    size,
                    radii,
                    with_alpha(colors.content, opacity),
                );
            }
        }
        match self.kind {
            SegmentKind::Leading => {
                if let Some(pod) = self.icon.as_mut() {
                    pod.paint_child(ctx, scene);
                }
                self.label.paint(
                    Point::new(
                        origin.x + self.label_origin.x,
                        origin.y + self.label_origin.y,
                    ),
                    colors.content,
                    scene,
                );
            }
            SegmentKind::Trailing => {
                if let Some(path) = self.chevron_path(size) {
                    scene.fill_path(origin, &path, &Brush::Solid(colors.content));
                }
            }
        }
        scene.pop_clip();

        if let Some(outline) = colors.outline {
            let path = inset_stroke_rrect(size, radii, OUTLINE_WIDTH).to_path(PATH_TOLERANCE);
            scene.stroke_path(origin, &path, OUTLINE_WIDTH, &Brush::Solid(outline));
        }
        if self.state.focused {
            let ring = focus_ring_rrect(size, radii).to_path(PATH_TOLERANCE);
            scene.stroke_path(
                origin,
                &ring,
                FOCUS_RING_WIDTH,
                &Brush::Solid(colors.focus_ring),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.enabled {
            // The reference's `IgnorePointer(ignoring: !enabled)` plus each
            // segment's own null `onTap`/`canRequestFocus: false`.
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => {
                if key.repeat || !Self::activates(&key.key) {
                    return EventResult::Ignored;
                }
                self.activate(ctx);
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    self.state.set_pressed(true);
                    self.captured = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let over = inside(p.position, ctx.size());
                    if !self.captured {
                        if over {
                            ctx.claim_hover();
                            ctx.set_cursor(CursorIcon::Pointer);
                        }
                        if self.state.set_hovered(over) {
                            ctx.request_redraw();
                        }
                        return EventResult::Ignored;
                    }
                    ctx.set_cursor(CursorIcon::Pointer);
                    if self.state.set_pressed(over) {
                        ctx.request_redraw();
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    if inside(p.position, ctx.size()) {
                        self.activate(ctx);
                    }
                    self.state.set_pressed(false);
                    self.captured = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    self.state.set_pressed(false);
                    self.captured = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Each half is its own `Role::Button` node, so a screen reader reaches
        // the action and the menu trigger independently — the semantics mirror
        // of the two focus nodes (the [module docs](self)' Two segments
        // section). The leading icon child is decoration and is not forwarded,
        // matching the plain button.
        ctx.push_node(Role::Button, |node| {
            if self.is_trailing() {
                // The carried name (`SplitButtonView::trailing_label`, already
                // resolved by `trailing_view`) wins; `DEFAULT_TRAILING_LABEL`
                // is only the last-resort fallback for an explicitly-empty
                // custom label.
                node.set_label(if self.label.content().is_empty() {
                    DEFAULT_TRAILING_LABEL
                } else {
                    self.label.content()
                });
                node.set_expanded(self.open);
            } else if !self.label.content().is_empty() {
                node.set_label(self.label.content());
            }
            if self.enabled {
                node.add_action(Action::Click);
                if self.is_trailing() {
                    node.add_action(Action::Expand);
                }
            } else {
                node.set_disabled();
            }
        });
    }

    visit_children!(icon);
}

// ---- The declarative view ---------------------------------------------------

/// Build one segment's retained pod. Built directly rather than through
/// [`frust::authoring::build_child`]'s [`AnyView`] route so the pod holds a
/// concrete [`SegmentWidget`] the container can downcast back to — the
/// [`mod@crate::navigation_rail`] destination-pod precedent.
fn build_segment<State: 'static>(view: &SegmentView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    ChildPod::new(Box::new(View::<State>::build(view, ctx)))
}

/// The concrete segment inside `pod`.
fn segment_of(pod: &mut ChildPod) -> &mut SegmentWidget {
    pod.widget_mut()
        .downcast_mut::<SegmentWidget>()
        .expect("a split button pod holds a SegmentWidget")
}

/// A view-held, typed callback (erased on build).
type Callback<State> = Rc<dyn Fn(&mut State)>;
/// A view-held, typed menu-selection callback.
type OnSelect<State> = Rc<dyn Fn(&mut State, MenuSelection)>;
/// A view-held leading-icon factory: the icon view is rebuilt per pass, so it
/// cannot be stored as a single `AnyView`.
type IconFactory<State> = Rc<dyn Fn() -> AnyView<State>>;

/// A declarative split button. See the [module docs](self).
pub struct SplitButtonView<State: 'static> {
    label: String,
    trailing_label: Option<String>,
    size: SplitButtonSize,
    shape: SplitButtonShape,
    variant: SplitButtonVariant,
    trailing_alignment: SplitButtonTrailingAlignment,
    enabled: bool,
    trailing_enabled: bool,
    /// The controlled expanded state driving the chevron and the open shape.
    open: bool,
    gap: Option<f64>,
    haptic: HapticSignal,
    leading_icon: Option<IconFactory<State>>,
    items: Vec<SplitButtonItem>,
    selected_value: Option<String>,
    menu_style: SplitButtonMenuStyle,
    menu_anchor: OverlayAnchor,
    on_press: Callback<State>,
    on_open: Callback<State>,
    on_select: Option<OnSelect<State>>,
}

/// Create a split button labelled `label`, with a leading `on_press` action and
/// a trailing `on_open` menu callback.
///
/// `open` reflects whether the menu the app presents is currently showing: it
/// drives the chevron's rotation and the trailing half's open shape, and — as a
/// *controlled* component — it moves only when the app feeds a new value back
/// (typically toggled inside `on_open`), never by itself. Pass `false` for a
/// plain action+menu split button that never rotates.
///
/// Chain [`SplitButtonView::size`]/[`SplitButtonView::variant`]/
/// [`SplitButtonView::shape`] for the reference's other tiers,
/// [`SplitButtonView::items`] plus [`SplitButtonView::on_select`] to describe
/// the menu, and [`SplitButtonView::menu_route`] to get the presentation the
/// chosen [`SplitButtonView::menu_style`] calls for.
pub fn split_button<State, P, O>(
    label: impl Into<String>,
    open: bool,
    on_press: P,
    on_open: O,
) -> SplitButtonView<State>
where
    State: 'static,
    P: Fn(&mut State) + 'static,
    O: Fn(&mut State) + 'static,
{
    SplitButtonView {
        label: label.into(),
        trailing_label: None,
        size: SplitButtonSize::default(),
        shape: SplitButtonShape::default(),
        variant: SplitButtonVariant::default(),
        trailing_alignment: SplitButtonTrailingAlignment::default(),
        enabled: true,
        trailing_enabled: true,
        open,
        gap: None,
        haptic: HapticSignal::None,
        leading_icon: None,
        items: Vec::new(),
        selected_value: None,
        menu_style: SplitButtonMenuStyle::default(),
        menu_anchor: OverlayAnchor::new(),
        on_press: Rc::new(on_press),
        on_open: Rc::new(on_open),
        on_select: None,
    }
}

/// PascalCase alias for [`split_button`], matching the catalog's view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn SplitButton<State, P, O>(
    label: impl Into<String>,
    open: bool,
    on_press: P,
    on_open: O,
) -> SplitButtonView<State>
where
    State: 'static,
    P: Fn(&mut State) + 'static,
    O: Fn(&mut State) + 'static,
{
    split_button(label, open, on_press, on_open)
}

impl<State: 'static> SplitButtonView<State> {
    /// Set the size tier (the [module docs](self)' Sizes table).
    pub fn size(mut self, size: SplitButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set the corner-radius family (pill vs. per-size token radius).
    pub fn shape(mut self, shape: SplitButtonShape) -> Self {
        self.shape = shape;
        self
    }

    /// Set the container treatment (the [module docs](self)' Variants section).
    pub fn variant(mut self, variant: SplitButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set how the trailing chevron is centred (`trailingAlignment`).
    pub fn trailing_alignment(mut self, alignment: SplitButtonTrailingAlignment) -> Self {
        self.trailing_alignment = alignment;
        self
    }

    /// Whether the control as a whole accepts input — the reference's
    /// `IgnorePointer(ignoring: !enabled)`. `false` disables **both** halves
    /// regardless of [`Self::trailing_enabled`].
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Whether the trailing menu trigger accepts input, independently of the
    /// leading action (default `true`).
    ///
    /// The reference carries `leadingEnabled`/`trailingEnabled` as two distinct
    /// variables fed from one prop (`m3e_split_button_content.dart:212`); this
    /// makes the second one real, so an app can offer the action while the menu
    /// has nothing to show. A disabled trailing half paints the 38%/12%
    /// disabled roles, reports disabled semantics, and simply never arms.
    pub fn trailing_enabled(mut self, enabled: bool) -> Self {
        self.trailing_enabled = enabled;
        self
    }

    /// Attach a leading icon before the label (`M3ESplitButton.leadingIcon`).
    /// The factory is re-invoked per pass, since a view is a value rebuilt every
    /// frame.
    pub fn leading_icon<F: Fn() -> AnyView<State> + 'static>(mut self, icon: F) -> Self {
        self.leading_icon = Some(Rc::new(icon));
        self
    }

    /// Override the trailing half's accessible name (default
    /// `"Show menu"`) — the reference's `trailingTooltip` in the one role this
    /// framework has for it.
    pub fn trailing_label(mut self, label: impl Into<String>) -> Self {
        self.trailing_label = Some(label.into());
        self
    }

    /// Override the gap between the halves, in logical px
    /// (`M3ESplitButtonDecoration.gap`). Unset resolves
    /// [`SPLIT_ELEVATED_INNER_GAP`] for [`SplitButtonVariant::Elevated`] and
    /// [`SPLIT_INNER_GAP`] otherwise.
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = Some(gap);
        self
    }

    /// The haptic signal both halves fire on activation. Defaults to
    /// [`HapticSignal::None`], matching the reference (a button is silent unless
    /// its decoration names a signal).
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// The menu's items (`M3ESplitButton.items`) — the list both
    /// [`SplitButtonMenuStyle`]s present.
    pub fn items(mut self, items: Vec<SplitButtonItem>) -> Self {
        self.items = items;
        self
    }

    /// The app-confirmed selected value the menu's rows check against
    /// (`M3ESplitButton.selectedValue`).
    pub fn selected_value(mut self, value: Option<String>) -> Self {
        self.selected_value = value;
        self
    }

    /// Which presentation the menu takes (`M3ESplitButtonDecoration.menuStyle`).
    pub fn menu_style(mut self, style: SplitButtonMenuStyle) -> Self {
        self.menu_style = style;
        self
    }

    /// Fired when a menu row is activated (`M3ESplitButton.onSelected`).
    pub fn on_select<F: Fn(&mut State, MenuSelection) + 'static>(mut self, on_select: F) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// Capture the **trailing segment's** window rect into `anchor` on every
    /// paint, so [`Self::popup_menu`] has a rect to place against — the same
    /// [`OverlayAnchor`] handoff
    /// [`crate::button_group::ButtonGroupView::overflow_anchor`] takes.
    pub fn menu_anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.menu_anchor = anchor.clone();
        self
    }

    /// Whether the trailing half is live: the whole control's gate AND its own.
    fn trailing_is_enabled(&self) -> bool {
        self.enabled && self.trailing_enabled
    }

    /// The resolved gap between the halves, before the focus-ring widening the
    /// widget's own `layout` adds (`_computeSegmentGeometry`'s `baseGap`).
    fn base_gap(&self) -> f64 {
        self.gap
            .unwrap_or(if self.variant == SplitButtonVariant::Elevated {
                SPLIT_ELEVATED_INNER_GAP
            } else {
                SPLIT_INNER_GAP
            })
    }

    /// The item list as menu nodes.
    fn menu_nodes(&self) -> Vec<MenuNode> {
        self.items
            .iter()
            .map(SplitButtonItem::to_menu_node)
            .collect()
    }

    /// The popup-style menu — a [`crate::menu()`] anchored below the trailing
    /// segment. Mount it separately (the top of the app's own [`frust::Stack`],
    /// or a transparent navigator page) and keep it mounted, toggling the
    /// `open` flag [`split_button`] takes rather than unmounting it, for the
    /// host's exit ramp. See the [module docs](self)' Menu styles section.
    pub fn popup_menu(&self) -> AnyView<State> {
        let on_select = self.on_select.clone();
        any(
            menu(self.menu_nodes(), move |state: &mut State, selection| {
                if let Some(callback) = &on_select {
                    callback(state, selection);
                }
            })
            .anchor(&self.menu_anchor)
            .selected(self.selected_value.clone())
            .open(self.open),
        )
    }

    /// The bottom-sheet-style menu — the same items inside a
    /// [`crate::bottom_sheet`], to hand to [`crate::show_bottom_sheet`] (which
    /// needs a [`frust::NavigatorController`] this widget cannot reach). See the
    /// [module docs](self)' Menu styles section.
    pub fn sheet_menu(&self) -> BottomSheetView<State> {
        let on_select = self.on_select.clone();
        bottom_sheet(
            menu_panel(self.menu_nodes(), move |state: &mut State, selection| {
                if let Some(callback) = &on_select {
                    callback(state, selection);
                }
            })
            .selected(self.selected_value.clone()),
        )
    }

    /// The presentation this button's [`Self::menu_style`] calls for — the
    /// route selector `_openMenu` performs (`m3e_split_button_menu.dart:22`),
    /// minus the descoped `native` arm.
    pub fn menu_route(&self) -> SplitButtonMenuRoute<State> {
        match self.menu_style {
            SplitButtonMenuStyle::Popup => SplitButtonMenuRoute::Popup(self.popup_menu()),
            SplitButtonMenuStyle::BottomSheet => SplitButtonMenuRoute::Sheet(self.sheet_menu()),
        }
    }

    /// The leading segment's view for this pass.
    fn leading_view(&self) -> SegmentView<State> {
        let on_press = self.on_press.clone();
        SegmentView {
            kind: SegmentKind::Leading,
            size: self.size,
            shape: self.shape,
            variant: self.variant,
            enabled: self.enabled,
            open: self.open,
            trailing_alignment: self.trailing_alignment,
            label: self.label.clone(),
            icon: self.leading_icon.as_ref().map(|factory| factory()),
            haptic: self.haptic,
            anchor: None,
            on_press,
        }
    }

    /// The trailing segment's view for this pass.
    fn trailing_view(&self) -> SegmentView<State> {
        let on_open = self.on_open.clone();
        SegmentView {
            kind: SegmentKind::Trailing,
            size: self.size,
            shape: self.shape,
            variant: self.variant,
            enabled: self.trailing_is_enabled(),
            open: self.open,
            trailing_alignment: self.trailing_alignment,
            label: self
                .trailing_label
                .clone()
                .unwrap_or_else(|| DEFAULT_TRAILING_LABEL.to_string()),
            icon: None,
            haptic: self.haptic,
            anchor: Some(self.menu_anchor.clone()),
            on_press: on_open,
        }
    }
}

/// The retained widget for a [`SplitButtonView`]: the two segment pods plus the
/// gap between them. It paints nothing of its own — each segment owns its
/// container, clip, state layer and content.
pub struct SplitButtonWidget {
    /// `[leading, trailing]`, in paint order.
    segments: Vec<ChildPod>,
    size: SplitButtonSize,
    base_gap: f64,
    /// The gap this frame's layout resolved, focus widening included.
    gap: f64,
}

impl SplitButtonWidget {
    /// The gap to lay out with — `baseGap` plus [`SPLIT_FOCUS_RING_OUTSET`]
    /// while **either** half holds focus, so a ring never overlaps its
    /// neighbour (`_computeSegmentGeometry`'s `eitherFocused`).
    fn resolved_gap(&self) -> f64 {
        let focused = self.segments.iter().any(ChildPod::is_focused);
        self.base_gap
            + if focused {
                SPLIT_FOCUS_RING_OUTSET
            } else {
                0.0
            }
    }
}

impl<State: 'static> View<State> for SplitButtonView<State> {
    type Element = SplitButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SplitButtonWidget {
        SplitButtonWidget {
            segments: vec![
                build_segment(&self.leading_view(), ctx),
                build_segment(&self.trailing_view(), ctx),
            ],
            size: self.size,
            base_gap: self.base_gap(),
            gap: self.base_gap(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SplitButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let base_gap = self.base_gap();
        if element.base_gap != base_gap {
            element.base_gap = base_gap;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags |= View::<State>::rebuild(
            &self.leading_view(),
            &prev.leading_view(),
            segment_of(&mut element.segments[0]),
            ctx,
        );
        flags |= View::<State>::rebuild(
            &self.trailing_view(),
            &prev.trailing_view(),
            segment_of(&mut element.segments[1]),
            ctx,
        );
        flags
    }

    fn teardown(&self, element: &mut SplitButtonWidget, ctx: &mut BuildCtx<'_>) {
        View::<State>::teardown(
            &self.leading_view(),
            segment_of(&mut element.segments[0]),
            ctx,
        );
        View::<State>::teardown(
            &self.trailing_view(),
            segment_of(&mut element.segments[1]),
            ctx,
        );
    }
}

impl Widget for SplitButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let gap = self.resolved_gap();
        self.gap = gap;
        let segment_height = self.size.height();
        // `ConstrainedBox(minHeight: minTapTarget)` around the row, with each
        // segment centred inside it (`_wrapSegmentChrome`'s `Center`).
        let height = segment_height.max(SPLIT_MIN_TAP_TARGET);

        // The trailing half self-sizes off its own size row and open state, so
        // it measures first and the leading half gets whatever is left.
        let trailing = self.segments[1].layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(bc.max().width, segment_height)),
        );
        let leading_max = if bc.max().width.is_finite() {
            (bc.max().width - gap - trailing.width).max(0.0)
        } else {
            f64::INFINITY
        };
        let leading = self.segments[0].layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(leading_max, segment_height)),
        );

        self.segments[0].set_origin(Point::new(0.0, (height - leading.height) / 2.0));
        self.segments[1].set_origin(Point::new(
            leading.width + gap,
            (height - trailing.height) / 2.0,
        ));
        bc.constrain(Size::new(leading.width + gap + trailing.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.segments {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The standard container discipline: broadcast to every child first,
        // then the capture path, then the focus path, then a topmost-first hit
        // test. The gap between the halves belongs to neither segment and is
        // inert, exactly like the reference's `SizedBox`.
        route_event(&mut self.segments, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One `Role::Group` (the reference's `FocusTraversalGroup`) over the two
        // segments' own nodes — each half reports itself, including its own
        // disabled state and the trailing half's expanded state.
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                for pod in &self.segments {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(segments);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{KeyEvent, Modifiers, PointerButton, PointerEvent};
    use std::any::Any;

    // ---- fixtures ---------------------------------------------------------

    #[derive(Default)]
    struct Log {
        presses: u32,
        opens: u32,
        selections: Vec<MenuSelection>,
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn build<S: 'static>(view: &SplitButtonView<S>) -> SplitButtonWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild<S: 'static>(
        prev: &SplitButtonView<S>,
        next: &SplitButtonView<S>,
        widget: &mut SplitButtonWidget,
    ) {
        let mut counter = 0u64;
        View::<S>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut counter));
    }

    fn plain(open: bool) -> SplitButtonView<Log> {
        split_button::<Log, _, _>(
            "Save",
            open,
            |s: &mut Log| s.presses += 1,
            |s: &mut Log| s.opens += 1,
        )
    }

    fn layout_with(widget: &mut SplitButtonWidget, max_width: f64, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        if let Some(theme) = theme {
            lctx = lctx.with_theme(theme as &dyn Any);
        }
        widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(max_width, 500.0)),
        )
    }

    fn laid_out(view: &SplitButtonView<Log>) -> SplitButtonWidget {
        let mut widget = build(view);
        layout_with(&mut widget, 600.0, None);
        widget
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(widget: &mut SplitButtonWidget, state: &mut Log, size: Size, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    /// A click at `(x, y)` in the widget's own local space.
    fn click(widget: &mut SplitButtonWidget, state: &mut Log, size: Size, x: f64, y: f64) {
        dispatch(widget, state, size, &ev(PointerPhase::Down, x, y));
        dispatch(widget, state, size, &ev(PointerPhase::Up, x, y));
    }

    fn segment(widget: &mut SplitButtonWidget, index: usize) -> &mut SegmentWidget {
        segment_of(&mut widget.segments[index])
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, CornerRadii, Color)>,
        clips: Vec<(Point, Size, CornerRadii)>,
        pops: usize,
        paths: Vec<Color>,
        strokes: Vec<(f64, Color)>,
        runs: Vec<(Point, Color)>,
        shadows: usize,
    }

    fn solid(brush: &Brush) -> Color {
        match brush {
            Brush::Solid(c) => *c,
            _ => Color::TRANSPARENT,
        }
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.rrects.push((o, s, radii, color));
        }
        fn push_clip_rounded_radii(&mut self, o: Point, s: Size, radii: CornerRadii) {
            self.clips.push((o, s, radii));
        }
        fn pop_clip(&mut self) {
            self.pops += 1;
        }
        fn fill_path(&mut self, _o: Point, _p: &BezPath, brush: &Brush) {
            self.paths.push(solid(brush));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, width: f64, brush: &Brush) {
            self.strokes.push((width, solid(brush)));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.runs.push((Point::new(t.x, t.y), solid(&run.brush)));
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _sd: f64, _c: Color) {
            self.shadows += 1;
        }
    }

    fn paint_at(widget: &mut SplitButtonWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        if let Some(theme) = theme {
            pctx = pctx.with_theme(theme as &dyn Any);
        }
        widget.paint(&mut pctx, &mut rec);
        rec
    }

    /// Advance a segment's shape morph past its settling point.
    fn settle_morph(segment: &mut SegmentWidget) {
        let mut t = 0.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !segment.motion.advance(ft_secs(t)) {
                return;
            }
        }
        panic!("the shape morph never settled");
    }

    // ---- 1. the size / variant matrix -------------------------------------

    #[test]
    fn every_size_row_matches_the_reference_theme_tables() {
        // m3e_split_button_theme.dart:76 (_splitHeight), :84
        // (_splitTrailingWidth), :164 (_splitLeftOuterPadding), :180
        // (_splitLabelRightPadding), :124 (_splitIcon).
        let table = [
            (SplitButtonSize::Xs, 32.0, 22.0, 12.0, 10.0, 20.0),
            (SplitButtonSize::Sm, 40.0, 22.0, 16.0, 12.0, 24.0),
            (SplitButtonSize::Md, 56.0, 26.0, 24.0, 24.0, 24.0),
            (SplitButtonSize::Lg, 96.0, 38.0, 48.0, 48.0, 32.0),
            (SplitButtonSize::Xl, 136.0, 50.0, 64.0, 64.0, 40.0),
        ];
        for (size, height, chevron, left, right, icon) in table {
            assert_eq!(size.height(), height, "{size:?} height");
            assert_eq!(size.trailing_icon_size(), chevron, "{size:?} chevron");
            assert_eq!(size.left_outer_padding(), left, "{size:?} left pad");
            assert_eq!(size.label_right_padding(), right, "{size:?} right pad");
            assert_eq!(size.icon_size(), icon, "{size:?} icon");
        }
        assert_eq!(SplitButtonSize::default(), SplitButtonSize::Sm);
        assert_eq!(SplitButtonSize::ALL.len(), 5);
    }

    #[test]
    fn the_three_trailing_width_formulas_agree() {
        // `_trailingPadsAndWidth` resolves three widths (unselected, selected,
        // circle); every one of them lands on the same number for every size,
        // which is why the laid-out trailing width never moves when the menu
        // opens.
        for size in SplitButtonSize::ALL {
            assert_eq!(
                size.trailing_width_unselected(),
                size.trailing_width_selected(),
                "{size:?}"
            );
            if size.allows_circle_trailing() {
                assert_eq!(
                    size.trailing_width_unselected(),
                    size.height(),
                    "{size:?} circle width is the segment height"
                );
            }
            assert_eq!(
                size.trailing_width(),
                size.trailing_width_unselected().max(SPLIT_MIN_TAP_TARGET)
            );
        }
        assert_eq!(SplitButtonSize::Xs.trailing_width(), 48.0);
        assert_eq!(SplitButtonSize::Md.trailing_width(), 56.0);
        assert_eq!(SplitButtonSize::Xl.trailing_width(), 136.0);
    }

    #[test]
    fn each_size_lays_its_segments_out_at_the_table_height_and_trailing_width() {
        for size in SplitButtonSize::ALL {
            let mut widget = laid_out(&plain(false).size(size));
            let expected_height = size.height().max(SPLIT_MIN_TAP_TARGET);
            let outer = layout_with(&mut widget, 600.0, None);
            assert_eq!(outer.height, expected_height, "{size:?} outer height");
            assert_eq!(
                widget.segments[0].size().height,
                size.height(),
                "{size:?} leading segment height"
            );
            assert_eq!(
                widget.segments[1].size().width,
                size.trailing_width(),
                "{size:?} trailing segment width"
            );
            // The two halves abut across exactly the resolved gap.
            let leading_right = widget.segments[0].size().width;
            assert_eq!(
                widget.segments[1].origin().x,
                leading_right + SPLIT_INNER_GAP,
                "{size:?} gap"
            );
        }
    }

    #[test]
    fn each_variant_resolves_its_own_container_and_content_roles() {
        // m3e_button_theme.dart:111/:127, reached through
        // `_resolveColorsAndShapes`.
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let s = theme.scheme();
        let expected = [
            (SplitButtonVariant::Filled, s.primary, s.on_primary, false),
            (
                SplitButtonVariant::Tonal,
                s.secondary_container,
                s.on_secondary_container,
                false,
            ),
            (
                SplitButtonVariant::Elevated,
                s.surface_container_low,
                s.primary,
                false,
            ),
            (
                SplitButtonVariant::Outlined,
                Color::TRANSPARENT,
                s.primary,
                true,
            ),
        ];
        for (variant, container, content, outlined) in expected {
            let colors = resolve_colors(Some(&theme), variant, true);
            assert_eq!(colors.container, container, "{variant:?} container");
            assert_eq!(colors.content, content, "{variant:?} content");
            assert_eq!(colors.outline.is_some(), outlined, "{variant:?} outline");
            assert_eq!(colors.focus_ring, s.primary, "{variant:?} focus ring");
        }
        assert_eq!(SplitButtonVariant::default(), SplitButtonVariant::Filled);
        assert_eq!(SplitButtonVariant::ALL.len(), 4);
    }

    #[test]
    fn the_unthemed_fallbacks_are_the_m3_baseline_light_values() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        for variant in SplitButtonVariant::ALL {
            for enabled in [true, false] {
                assert_eq!(
                    resolve_colors(None, variant, enabled),
                    resolve_colors(Some(&light), variant, enabled),
                    "{variant:?} enabled={enabled}"
                );
            }
        }
    }

    #[test]
    fn a_disabled_segment_takes_the_reference_disabled_alphas() {
        // m3e_split_button_style.dart:129 — content 38%, opaque container 12%,
        // over `on_surface`; a transparent (outlined) container stays clear.
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let on_surface = theme.scheme().on_surface;
        let filled = resolve_colors(Some(&theme), SplitButtonVariant::Filled, false);
        assert_eq!(
            filled.content,
            with_alpha(on_surface, DISABLED_CONTENT_OPACITY)
        );
        assert_eq!(
            filled.container,
            with_alpha(on_surface, DISABLED_CONTAINER_OPACITY)
        );
        let outlined = resolve_colors(Some(&theme), SplitButtonVariant::Outlined, false);
        assert_eq!(outlined.container, Color::TRANSPARENT);
        assert_eq!(
            outlined.outline,
            Some(with_alpha(on_surface, DISABLED_CONTAINER_OPACITY))
        );
    }

    #[test]
    fn the_gap_is_wider_for_the_elevated_variant() {
        // `_computeSegmentGeometry`'s `baseGap`.
        assert_eq!(plain(false).base_gap(), SPLIT_INNER_GAP);
        assert_eq!(
            plain(false)
                .variant(SplitButtonVariant::Elevated)
                .base_gap(),
            SPLIT_ELEVATED_INNER_GAP
        );
        assert_eq!(plain(false).gap(9.0).base_gap(), 9.0);
        assert_eq!(SPLIT_INNER_GAP, 2.0);
        assert_eq!(SPLIT_ELEVATED_INNER_GAP, 4.0);
        assert_eq!(SPLIT_MIN_TAP_TARGET, 48.0);
    }

    // ---- 2. connected geometry (per-corner radii) --------------------------

    #[test]
    fn the_resting_connected_radii_are_outer_outside_and_inner_at_the_seam() {
        for size in SplitButtonSize::ALL {
            let mut widget = laid_out(&plain(false).size(size));
            let pill = size.height() / 2.0;
            let inner = size.inner_radius(None);

            let leading = segment(&mut widget, 0).target_radii(None, size.height());
            assert_eq!(
                leading,
                CornerRadii::new(pill, inner, inner, pill),
                "{size:?}"
            );
            let trailing = segment(&mut widget, 1).target_radii(None, size.height());
            assert_eq!(
                trailing,
                CornerRadii::new(inner, pill, pill, inner),
                "{size:?}"
            );
        }
    }

    #[test]
    fn the_square_shape_family_swaps_the_outer_corners_for_the_token_radius() {
        for size in SplitButtonSize::ALL {
            let mut widget = laid_out(&plain(false).size(size).shape(SplitButtonShape::Square));
            let outer = size.square_outer_radius();
            let inner = size.inner_radius(None);
            assert_eq!(
                segment(&mut widget, 0).target_radii(None, size.height()),
                CornerRadii::new(outer, inner, inner, outer),
                "{size:?}"
            );
        }
        assert_eq!(SplitButtonShape::default(), SplitButtonShape::Round);
    }

    #[test]
    fn the_inner_radius_column_resolves_from_the_shape_scale() {
        // 4/4/4/8/12 lands exactly on extra_small/small/medium.
        let theme = crate::baseline();
        assert_eq!(
            SplitButtonSize::Sm.inner_radius(Some(&theme)),
            theme.shape.extra_small
        );
        assert_eq!(
            SplitButtonSize::Lg.inner_radius(Some(&theme)),
            theme.shape.small
        );
        assert_eq!(
            SplitButtonSize::Xl.inner_radius(Some(&theme)),
            theme.shape.medium
        );
        // ...and the literal fallback is the same table.
        assert_eq!(SplitButtonSize::Sm.inner_radius(None), 4.0);
        assert_eq!(SplitButtonSize::Lg.inner_radius(None), 8.0);
        assert_eq!(SplitButtonSize::Xl.inner_radius(None), 12.0);
        // The hovered / pressed / square-outer columns stay literal.
        assert_eq!(SplitButtonSize::Sm.hovered_inner_radius(), 12.0);
        assert_eq!(SplitButtonSize::Sm.pressed_radius(), 2.0);
        assert_eq!(SplitButtonSize::Sm.square_outer_radius(), 10.0);
        assert_eq!(SplitButtonSize::Xl.hovered_inner_radius(), 20.0);
        assert_eq!(SplitButtonSize::Xl.pressed_radius(), 6.0);
    }

    #[test]
    fn pressed_outranks_hovered_at_the_seam() {
        let mut widget = laid_out(&plain(false));
        let size = SplitButtonSize::Sm;
        let seg = segment(&mut widget, 0);
        seg.state.set_hovered(true);
        assert_eq!(
            seg.target_radii(None, size.height()).top_right,
            size.hovered_inner_radius()
        );
        seg.state.set_pressed(true);
        assert_eq!(
            seg.target_radii(None, size.height()).top_right,
            size.pressed_radius(),
            "pressed outranks hovered"
        );
        // A disabled segment resolves neither.
        seg.enabled = false;
        assert_eq!(
            seg.target_radii(None, size.height()).top_right,
            size.inner_radius(None)
        );
    }

    #[test]
    fn an_open_trailing_half_goes_fully_round_and_suppresses_hover() {
        let mut widget = laid_out(&plain(true).size(SplitButtonSize::Md));
        let height = SplitButtonSize::Md.height();
        let seg = segment(&mut widget, 1);
        seg.state.set_hovered(true);
        let radii = seg.target_radii(None, height);
        assert_eq!(
            radii,
            CornerRadii::uniform(height * TRAILING_SELECTED_CORNER_PERCENT / 100.0),
            "the open trailing half is a pill on every corner"
        );
        assert_eq!(TRAILING_SELECTED_CORNER_PERCENT, 50.0);
    }

    #[test]
    fn the_circle_trailing_branch_coincides_with_the_open_branch() {
        // `circleTrailing` only fires for a round shape, whose outer radius is
        // already `height / 2` — the very value `trailingSelectedRadius`
        // resolves to. Ported anyway; this pins that it changes nothing.
        for size in SplitButtonSize::ALL {
            let mut widget = laid_out(&plain(true).size(size));
            let seg = segment(&mut widget, 1);
            assert_eq!(seg.circle_trailing(), size.allows_circle_trailing());
            let height = size.height();
            let selected = height * TRAILING_SELECTED_CORNER_PERCENT / 100.0;
            assert_eq!(
                seg.target_radii(None, height),
                CornerRadii::uniform(selected),
                "{size:?}"
            );
        }
    }

    // ---- 3. the spring clip + chevron rotation -----------------------------

    #[test]
    fn the_shape_spring_matches_expressive_spatial_press() {
        let token = crate::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS;
        assert_eq!(SPLIT_SHAPE_SPRING.stiffness, token.stiffness);
        assert_eq!(SPLIT_SHAPE_SPRING.damping_ratio, token.damping_ratio);
        assert_eq!(SPLIT_SHAPE_SPRING.mass, 1.0);
        assert_eq!(SPLIT_SHAPE_SPRING.stiffness, 380.0);
        assert_eq!(SPLIT_SHAPE_SPRING.damping_ratio, 0.55);
    }

    #[test]
    fn the_shape_morph_snaps_on_mount_then_springs_and_stays_continuous() {
        let mut motion = ShapeMotion::new();
        let rest = CornerRadii::new(20.0, 4.0, 4.0, 20.0);
        assert!(
            !motion.retarget(rest),
            "the mount frame seeds, never animates"
        );
        assert_eq!(motion.radii(), rest);
        assert!(!motion.advance(ft_secs(0.0)), "nothing to advance");

        // A sub-tolerance retarget is inside the dead band.
        let nudged = CornerRadii::new(20.0 + RETARGET_TOLERANCE / 2.0, 4.0, 4.0, 20.0);
        assert!(!motion.retarget(nudged));
        assert_eq!(motion.target_radii(), rest);

        // A real one springs, starting where it stood.
        let pressed = CornerRadii::new(20.0, 2.0, 2.0, 20.0);
        assert!(motion.retarget(pressed));
        assert_eq!(motion.radii(), rest, "the leg starts where it was");
        motion.advance(ft_secs(0.0));
        motion.advance(ft_secs(0.05));
        let mid = motion.radii().top_right;
        assert!(mid < 4.0 && mid > 2.0, "sampled mid-flight (got {mid})");

        // Retargeting mid-flight paints exactly what the last frame painted.
        assert!(motion.retarget(rest));
        assert_eq!(motion.radii().top_right, mid);
        let mut t = 0.05;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            if !motion.advance(ft_secs(t)) {
                break;
            }
            assert!(motion.radii().top_right >= 0.0, "a corner went negative");
        }
        assert_eq!(motion.radii(), rest, "and settles exactly on target");
    }

    #[test]
    fn a_segment_clips_its_content_to_the_animating_radii() {
        let mut widget = laid_out(&plain(false));
        let rec = paint_at(&mut widget, Size::new(300.0, 48.0), None);
        assert_eq!(rec.clips.len(), 2, "one clip per segment");
        assert_eq!(rec.pops, 2, "every clip is popped");
        // The clip shape is the same one the container filled with.
        for (index, (clip_origin, clip_size, clip_radii)) in rec.clips.iter().enumerate() {
            let (fill_origin, fill_size, fill_radii, _) = rec.rrects[index];
            assert_eq!(*clip_origin, fill_origin);
            assert_eq!(*clip_size, fill_size);
            assert_eq!(*clip_radii, fill_radii);
        }
    }

    #[test]
    fn opening_the_menu_springs_the_trailing_clip_toward_the_pill() {
        let prev = plain(false).size(SplitButtonSize::Md);
        let mut widget = laid_out(&prev);
        paint_at(&mut widget, Size::new(300.0, 56.0), None);
        let closed = segment(&mut widget, 1).motion.target_radii();

        rebuild(&prev, &plain(true).size(SplitButtonSize::Md), &mut widget);
        paint_at(&mut widget, Size::new(300.0, 56.0), None);
        let seg = segment(&mut widget, 1);
        assert_ne!(seg.motion.target_radii(), closed, "the target moved");
        assert_eq!(seg.motion.target_radii(), CornerRadii::uniform(28.0));
        settle_morph(seg);
        assert_eq!(seg.motion.radii(), CornerRadii::uniform(28.0));
    }

    #[test]
    fn the_chevron_rotates_a_half_turn_on_open_over_the_reference_tween() {
        assert_eq!(CHEVRON_OPEN_TURNS, 0.5);
        assert_eq!(CHEVRON_ROTATION_DURATION, Duration::from_millis(120));

        // Geometric centring, so only the rotation moves the glyph (the
        // optical nudge has its own test below).
        let geometric = |open: bool| {
            plain(open).trailing_alignment(SplitButtonTrailingAlignment::GeometricCenter)
        };
        let prev = geometric(false);
        let mut widget = laid_out(&prev);
        assert_eq!(segment(&mut widget, 1).chevron_anim.value(), 0.0);

        let extent = Size::new(48.0, 40.0);
        let glyph_box = |w: &mut SplitButtonWidget| {
            segment(w, 1)
                .chevron_path(extent)
                .expect("the trailing chevron path")
                .bounding_box()
        };
        let closed = glyph_box(&mut widget);

        rebuild(&prev, &geometric(true), &mut widget);
        // Drive the tween to its end.
        let seg = segment(&mut widget, 1);
        let mut t = 0.0;
        for _ in 0..60 {
            t += 1.0 / 60.0;
            if !seg.chevron_anim.advance(ft_secs(t)) {
                break;
            }
        }
        assert_eq!(
            seg.chevron_anim.value(),
            1.0,
            "the tween lands on a half turn"
        );

        // A half turn about the segment's centre is a point reflection through
        // it: the open glyph sits exactly opposite the closed one.
        let open = glyph_box(&mut widget);
        let cx = extent.width / 2.0;
        let cy = extent.height / 2.0;
        assert!((open.center().x - (2.0 * cx - closed.center().x)).abs() < 1e-6);
        assert!((open.center().y - (2.0 * cy - closed.center().y)).abs() < 1e-6);
        assert!(
            closed.center().y > open.center().y,
            "the closed chevron hangs below the open one"
        );
    }

    #[test]
    fn the_chevron_takes_the_optical_nudge_only_while_closed() {
        // `_computeTrailingGeometry`'s `iconOffsetBase`.
        assert_eq!(SplitButtonSize::Md.menu_icon_offset(), -2.0);
        assert_eq!(SplitButtonSize::Xl.menu_icon_offset(), -6.0);

        let closed = laid_out(&plain(false).size(SplitButtonSize::Md));
        let mut closed = closed;
        let box_closed = segment(&mut closed, 1)
            .chevron_path(Size::new(56.0, 56.0))
            .expect("a chevron")
            .bounding_box();

        let mut geometric = laid_out(
            &plain(false)
                .size(SplitButtonSize::Md)
                .trailing_alignment(SplitButtonTrailingAlignment::GeometricCenter),
        );
        let box_geometric = segment(&mut geometric, 1)
            .chevron_path(Size::new(56.0, 56.0))
            .expect("a chevron")
            .bounding_box();

        assert!(
            (box_closed.center().x - (box_geometric.center().x - 2.0)).abs() < 1e-6,
            "the optical centre nudges the glyph leading by the size's offset"
        );
        assert_eq!(
            SplitButtonTrailingAlignment::default(),
            SplitButtonTrailingAlignment::OpticalCenter
        );
    }

    // ---- 4. focus-separated segments ---------------------------------------

    #[test]
    fn each_half_is_its_own_hit_target() {
        let mut widget = laid_out(&plain(false));
        let size = layout_with(&mut widget, 600.0, None);
        let leading_center = Point::new(widget.segments[0].size().width / 2.0, size.height / 2.0);
        let trailing_center = Point::new(
            widget.segments[1].origin().x + widget.segments[1].size().width / 2.0,
            size.height / 2.0,
        );

        let mut state = Log::default();
        click(
            &mut widget,
            &mut state,
            size,
            leading_center.x,
            leading_center.y,
        );
        assert_eq!(state.presses, 1);
        assert_eq!(state.opens, 0, "the leading half never opens the menu");

        click(
            &mut widget,
            &mut state,
            size,
            trailing_center.x,
            trailing_center.y,
        );
        assert_eq!(state.opens, 1);
        assert_eq!(state.presses, 1, "the trailing half never fires the action");
    }

    #[test]
    fn a_key_activation_reaches_only_the_focused_half() {
        let mut widget = laid_out(&plain(false));
        let size = layout_with(&mut widget, 600.0, None);
        let mut state = Log::default();

        // No focus recorded anywhere: a focus-routed key reaches nobody.
        dispatch(&mut widget, &mut state, size, &key(NamedKey::Enter));
        assert_eq!((state.presses, state.opens), (0, 0));

        widget.segments[0].set_focused(true);
        dispatch(&mut widget, &mut state, size, &key(NamedKey::Enter));
        assert_eq!(
            (state.presses, state.opens),
            (1, 0),
            "the focused leading half activated"
        );

        widget.segments[0].set_focused(false);
        widget.segments[1].set_focused(true);
        dispatch(&mut widget, &mut state, size, &key(NamedKey::Enter));
        assert_eq!(
            (state.presses, state.opens),
            (1, 1),
            "focus moved, and so did the activation"
        );

        // Space activates too; an auto-repeat never does.
        dispatch(
            &mut widget,
            &mut state,
            size,
            &InputEvent::Key(KeyEvent {
                key: Key::Character(" ".to_string()),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(state.opens, 2);
        dispatch(
            &mut widget,
            &mut state,
            size,
            &InputEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: true,
            }),
        );
        assert_eq!(state.opens, 2, "an auto-repeat is not a fresh activation");
    }

    #[test]
    fn the_trailing_half_disables_independently_of_the_leading_one() {
        let view = plain(false).trailing_enabled(false);
        let mut widget = laid_out(&view);
        let size = layout_with(&mut widget, 600.0, None);
        let mut state = Log::default();

        let trailing_x = widget.segments[1].origin().x + widget.segments[1].size().width / 2.0;
        click(&mut widget, &mut state, size, trailing_x, size.height / 2.0);
        assert_eq!(state.opens, 0, "a disabled trailing half never arms");

        click(&mut widget, &mut state, size, 20.0, size.height / 2.0);
        assert_eq!(state.presses, 1, "the leading action is still live");

        // ...and the whole-control gate still wins over the trailing one.
        let mut both_off = laid_out(&plain(false).enabled(false).trailing_enabled(true));
        let size = layout_with(&mut both_off, 600.0, None);
        let mut state = Log::default();
        click(&mut both_off, &mut state, size, 20.0, size.height / 2.0);
        let trailing_x = both_off.segments[1].origin().x + 10.0;
        click(
            &mut both_off,
            &mut state,
            size,
            trailing_x,
            size.height / 2.0,
        );
        assert_eq!((state.presses, state.opens), (0, 0));
    }

    #[test]
    fn focus_on_either_half_widens_the_gap_by_the_ring_outset() {
        assert_eq!(SPLIT_FOCUS_RING_OUTSET, 4.0);
        let mut widget = laid_out(&plain(false));
        layout_with(&mut widget, 600.0, None);
        let unfocused = widget.gap;
        assert_eq!(unfocused, SPLIT_INNER_GAP);

        widget.segments[1].set_focused(true);
        layout_with(&mut widget, 600.0, None);
        assert_eq!(widget.gap, SPLIT_INNER_GAP + SPLIT_FOCUS_RING_OUTSET);
        assert_eq!(
            widget.segments[1].origin().x,
            widget.segments[0].size().width + widget.gap
        );
    }

    #[test]
    fn semantics_yields_a_group_over_two_independently_reporting_buttons() {
        fn logic(_s: &mut ()) -> SplitButtonView<()> {
            split_button::<(), _, _>("Save", true, |_s: &mut ()| {}, |_s: &mut ()| {})
                .trailing_enabled(false)
        }
        let mut root: frust_core::RenderRoot<(), SplitButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 80.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Group container node is contributed");
        assert_eq!(group.1.children().len(), 2);

        let leading = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Save"))
            .expect("the leading action node");
        assert!(leading.1.supports_action(Action::Click));
        assert!(!leading.1.is_disabled());

        let trailing = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some(DEFAULT_TRAILING_LABEL))
            .expect("the trailing menu node");
        assert_eq!(trailing.1.is_expanded(), Some(true));
        assert!(
            trailing.1.is_disabled(),
            "the trailing half reports its own disabled state"
        );
        assert!(!trailing.1.supports_action(Action::Expand));
    }

    #[test]
    fn trailing_label_reaches_the_trailing_nodes_semantics() {
        // Revert-verify: pre-fix, `trailing_label` was stored but dropped
        // before the trailing `SegmentWidget` ever saw it, so this node
        // always reported the hardcoded `DEFAULT_TRAILING_LABEL` regardless
        // of what was set here.
        fn logic(_s: &mut ()) -> SplitButtonView<()> {
            split_button::<(), _, _>("Save", true, |_s: &mut ()| {}, |_s: &mut ()| {})
                .trailing_label("More actions")
        }
        let mut root: frust_core::RenderRoot<(), SplitButtonView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 80.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let trailing = update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("More actions"))
            .expect("the custom trailing label reaches the trailing node");
        assert_eq!(trailing.1.is_expanded(), Some(true));
        assert!(
            update
                .nodes
                .iter()
                .all(|(_, n)| n.label() != Some(DEFAULT_TRAILING_LABEL)),
            "a custom trailing label replaces the default, not just adds to it"
        );
    }

    #[test]
    fn rebuild_with_a_changed_trailing_label_updates_the_node() {
        // A struct (not a bare `String`) so `logic`'s `&mut` parameter stays
        // clean of clippy's `ptr_arg` lint.
        struct LabelState {
            label: &'static str,
        }
        fn logic(s: &mut LabelState) -> SplitButtonView<LabelState> {
            split_button::<LabelState, _, _>(
                "Save",
                true,
                |_s: &mut LabelState| {},
                |_s: &mut LabelState| {},
            )
            .trailing_label(s.label)
        }
        let mut root: frust_core::RenderRoot<LabelState, SplitButtonView<LabelState>> =
            frust_core::RenderRoot::new();
        let mut state = LabelState { label: "First" };
        let mut tcx = TextContext::new();
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(400.0, 80.0), &mut tcx as &mut dyn Any);
        let before = root.semantics();
        assert!(
            before.nodes.iter().any(|(_, n)| n.label() == Some("First")),
            "the initial trailing label reaches the node"
        );

        state.label = "Second";
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(400.0, 80.0), &mut tcx as &mut dyn Any);
        let after = root.semantics();

        assert!(
            after.nodes.iter().any(|(_, n)| n.label() == Some("Second")),
            "the rebuilt trailing label reaches the node"
        );
        assert!(
            after.nodes.iter().all(|(_, n)| n.label() != Some("First")),
            "the stale label is gone after rebuild"
        );
    }

    // ---- 5. the two menu styles --------------------------------------------

    fn with_items(open: bool) -> SplitButtonView<Log> {
        plain(open)
            .items(vec![
                split_button_item("Save as").value("save-as"),
                split_button_item("Export").leading(icons::CHECK),
                split_button_item("Delete").enabled(false),
            ])
            .on_select(|s: &mut Log, selection| s.selections.push(selection))
    }

    #[test]
    fn the_menu_style_selects_the_route() {
        let popup = with_items(true);
        assert!(matches!(popup.menu_route(), SplitButtonMenuRoute::Popup(_)));
        assert_eq!(popup.menu_style, SplitButtonMenuStyle::Popup);
        assert_eq!(SplitButtonMenuStyle::default(), SplitButtonMenuStyle::Popup);

        let sheet = with_items(true).menu_style(SplitButtonMenuStyle::BottomSheet);
        assert!(matches!(sheet.menu_route(), SplitButtonMenuRoute::Sheet(_)));
    }

    #[test]
    fn items_map_onto_selectable_menu_nodes() {
        let view = with_items(true);
        let nodes = view.menu_nodes();
        assert_eq!(nodes.len(), 3);
        let labels: Vec<&str> = view.items.iter().map(SplitButtonItem::label).collect();
        assert_eq!(labels, ["Save as", "Export", "Delete"]);
        assert_eq!(view.items[0].resolved_value(), "save-as");
        assert_eq!(
            view.items[1].resolved_value(),
            "Export",
            "an item with no explicit value reports its label"
        );
        for node in &nodes {
            assert!(
                matches!(node, MenuNode::Selectable(_)),
                "every item maps onto `M3EMenuSelectable`"
            );
        }
        let MenuNode::Selectable(delete) = &nodes[2] else {
            panic!("a selectable node");
        };
        assert_eq!(delete.label(), "Delete");
        assert!(
            !nodes[2].is_activatable(),
            "a disabled item stays disabled in the menu"
        );
    }

    #[test]
    fn the_popup_route_places_its_panel_against_the_trailing_segment() {
        // The trailing segment captures its own window rect into the anchor on
        // every paint (the inlined `overlay_anchor` capture), which is what the
        // popup then places against.
        let anchor = OverlayAnchor::new();
        let view = with_items(true).menu_anchor(&anchor);
        let mut widget = laid_out(&view);
        let size = layout_with(&mut widget, 600.0, None);
        paint_at(&mut widget, size, None);

        let captured = anchor.rect();
        assert_eq!(captured.x0, widget.segments[1].origin().x);
        assert_eq!(captured.width(), widget.segments[1].size().width);
        assert!(captured.height() > 0.0);
    }

    #[test]
    fn a_menu_selection_reports_through_on_select() {
        // Mount the popup route the documented way and click its first row.
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(40.0, 100.0, 160.0, 140.0));
        let mut root: frust_core::RenderRoot<Log, frust::StackView<Log>> =
            frust_core::RenderRoot::new();
        let mut state = Log::default();
        let mut tcx = TextContext::new();
        let cell = anchor.clone();
        let mut logic = move |_s: &mut Log| {
            frust::stack().child(with_items(true).menu_anchor(&cell).popup_menu())
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // The first row's centre, per the anchored menu's documented placement
        // (below the trigger, leading edges flush, at the anchor gap).
        let x = 40.0 + crate::menu::MENU_MIN_WIDTH / 2.0;
        let y = 140.0 + crate::overlay::OVERLAY_ANCHOR_GAP + 8.0 + (4.0 + 48.0) / 2.0;
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            root.event(
                &mut state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }
        assert_eq!(state.selections.len(), 1);
        assert_eq!(state.selections[0].label, "Save as");
        assert_eq!(state.selections[0].value(), Some("save-as"));
    }

    #[test]
    fn the_sheet_route_carries_the_same_items() {
        // The sheet is content for `show_bottom_sheet`; what this pins is that
        // the same item list reaches it (the panel is the catalog's one
        // item-list surface, per the module docs' porting decision).
        let mut root: frust_core::RenderRoot<Log, BottomSheetView<Log>> =
            frust_core::RenderRoot::new();
        let mut state = Log::default();
        let mut tcx = TextContext::new();
        // Mount whatever the sheet-style route hands back, per pass.
        let mut logic = move |_s: &mut Log| {
            let view = with_items(true).menu_style(SplitButtonMenuStyle::BottomSheet);
            match view.menu_route() {
                SplitButtonMenuRoute::Sheet(sheet) => sheet,
                SplitButtonMenuRoute::Popup(_) => {
                    panic!("the bottom-sheet style routes to a sheet")
                }
            }
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        // Every row reaches the accessibility tree through the sheet.
        for label in ["Save as", "Export", "Delete"] {
            assert!(
                update.nodes.iter().any(|(_, n)| n.label() == Some(label)),
                "`{label}` is present in the sheet-style menu"
            );
        }
    }

    // ---- paint / consumer smoke -------------------------------------------

    #[test]
    fn a_themed_paint_fills_both_halves_with_the_variant_container() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut widget = laid_out(&plain(false).variant(SplitButtonVariant::Tonal));
        let rec = paint_at(&mut widget, Size::new(300.0, 48.0), Some(&theme));
        let container = theme.scheme().secondary_container;
        assert_eq!(rec.rrects[0].3, container, "the leading container");
        assert_eq!(rec.rrects[1].3, container, "the trailing container");
        assert!(!rec.runs.is_empty(), "the leading label painted");
        assert_eq!(
            rec.paths.len(),
            1,
            "exactly one chevron path, on the trailing half"
        );
        assert_eq!(rec.paths[0], theme.scheme().on_secondary_container);
    }

    #[test]
    fn an_outlined_split_button_strokes_a_hairline_per_half() {
        let mut widget = laid_out(&plain(false).variant(SplitButtonVariant::Outlined));
        let rec = paint_at(&mut widget, Size::new(300.0, 48.0), None);
        assert_eq!(rec.strokes.len(), 2, "one hairline per segment");
        for (width, _) in &rec.strokes {
            assert_eq!(*width, OUTLINE_WIDTH);
        }
        assert!(
            rec.rrects.iter().all(|(_, _, _, c)| c.components[3] > 0.0),
            "a transparent container is never filled"
        );
    }

    #[test]
    fn an_elevated_split_button_casts_a_shadow_per_half() {
        let mut widget = laid_out(&plain(false).variant(SplitButtonVariant::Elevated));
        let rec = paint_at(&mut widget, Size::new(300.0, 48.0), None);
        assert_eq!(rec.shadows, 2);
        assert_eq!(
            elevation_dp(SplitButtonVariant::Elevated, true, false, false),
            1.0
        );
        assert_eq!(
            elevation_dp(SplitButtonVariant::Elevated, true, false, true),
            3.0
        );
        assert_eq!(
            elevation_dp(SplitButtonVariant::Elevated, true, true, false),
            0.0
        );
        assert_eq!(
            elevation_dp(SplitButtonVariant::Elevated, false, false, false),
            0.0
        );
    }

    #[test]
    fn a_leading_icon_lays_out_before_the_label_at_the_size_s_gap() {
        let with_icon = plain(false)
            .size(SplitButtonSize::Md)
            .leading_icon(|| any::<Log, _>(frust::icon(icons::CHECK)));
        let mut widget = laid_out(&with_icon);
        layout_with(&mut widget, 600.0, None);
        let seg = segment(&mut widget, 0);
        let icon = seg.icon.as_ref().expect("the icon pod");
        assert_eq!(icon.origin().x, SplitButtonSize::Md.left_outer_padding());
        assert_eq!(icon.size(), Size::new(24.0, 24.0));
        assert_eq!(
            seg.label_origin.x,
            SplitButtonSize::Md.left_outer_padding()
                + 24.0
                + SplitButtonSize::Md.icon_to_label_gap()
        );
    }

    #[test]
    fn the_original_four_argument_surface_still_compiles_and_fires() {
        // The compatibility pin: the pre-rework call shape, unchanged.
        let view: SplitButtonView<Log> = split_button(
            "Save",
            false,
            |s: &mut Log| s.presses += 1,
            |s: &mut Log| s.opens += 1,
        );
        let _pascal: SplitButtonView<Log> = SplitButton(
            "Save",
            false,
            |s: &mut Log| s.presses += 1,
            |s: &mut Log| s.opens += 1,
        );
        let mut widget = laid_out(&view);
        let size = layout_with(&mut widget, 600.0, None);
        let mut state = Log::default();
        click(&mut widget, &mut state, size, 20.0, size.height / 2.0);
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn a_release_outside_the_armed_half_fires_nothing() {
        let mut widget = laid_out(&plain(false));
        let size = layout_with(&mut widget, 600.0, None);
        let mut state = Log::default();
        let trailing_x = widget.segments[1].origin().x + 10.0;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 20.0, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Move, trailing_x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, trailing_x, 20.0),
        );
        assert_eq!((state.presses, state.opens), (0, 0));
    }

    #[test]
    fn a_cancel_disarms_without_firing() {
        let mut widget = laid_out(&plain(false));
        let size = layout_with(&mut widget, 600.0, None);
        let mut state = Log::default();
        let trailing_x = widget.segments[1].origin().x + 10.0;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, trailing_x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, trailing_x, 20.0),
        );
        assert_eq!(state.opens, 0);
        assert!(!segment(&mut widget, 1).captured);
    }

    #[test]
    fn a_non_material_theme_still_resolves_every_role() {
        // An app is free to thread a bare theme; nothing here may panic.
        let bare = Theme::neutral();
        for variant in SplitButtonVariant::ALL {
            let _ = resolve_colors(Some(&bare), variant, true);
            let _ = resolve_colors(Some(&bare), variant, false);
        }
        for size in SplitButtonSize::ALL {
            let _ = size.inner_radius(Some(&bare));
            let _ = size.label_style(Some(&bare));
        }
        let mut widget = laid_out(&plain(true));
        let _ = paint_at(&mut widget, Size::new(300.0, 48.0), Some(&bare));
    }
}
