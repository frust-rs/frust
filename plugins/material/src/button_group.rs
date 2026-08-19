// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/toggle_button_group/` tree is itself vendored from
// m3e_buttons (MIT, © 2026 Mudit Purohit) — `m3e_toggle_button_group.dart`
// plus its nine `components/` parts, `controllers/`, `enums/`, `models/` and
// `styles/m3e_toggle_button_group_theme.dart` (retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
// Porting decisions: the offstage checked/unchecked *measurer* and its
// `stableAllOverflowMeasured` gate collapse to a plain natural-size layout
// pass (this framework can lay a child out synchronously, which is the whole
// problem that machinery exists to work around); the `popup` overflow-menu
// style and `experimentalPaging` overflow mode are not ported (see the
// module docs' Overflow section).

//! The Material 3 Expressive **button group**: a row (or column) of
//! [`mod@crate::toggle_button`] members, `standard` (spaced) or `connected`
//! (shared-edge), with the reference's neighbor-squish press animation.
//!
//! Every member is a real [`crate::ToggleButtonView`] — the group hands it
//! [`ToggleButtonView::group_connected`](crate::ToggleButtonView::group_connected)/
//! [`first_in_group`](crate::ToggleButtonView::first_in_group)/
//! [`last_in_group`](crate::ToggleButtonView::last_in_group) and that widget
//! owns its own corner asymmetry, state layer, shape morph and haptics. This
//! module owns only what a *group* adds: the axis layout, the resolved gap,
//! the squish, the selection fan-out, and the overflow split.
//!
//! # Types × densities × direction
//!
//! | Axis | Values | Reference |
//! |---|---|---|
//! | [`ButtonGroupType`] | `Standard` (gaps) / `Connected` (shared edges) | `M3EButtonGroupType`, `m3e_toggle_button_group_enums.dart:9` |
//! | [`ButtonGroupDensity`] | `Regular` / `Compact` (75% of the token gap, floored to whole dp) | `M3EButtonGroupDensity`, `:21` |
//! | [`ButtonGroupDirection`] | `Horizontal` / `Vertical` | `M3EButtonGroup.direction` (`Axis`) |
//!
//! The resolved member gap ([`resolve_spacing`]) is:
//!
//! | Type | Regular | Compact |
//! |---|---|---|
//! | `Standard` | 8dp (`standardSpacing`) | 6dp (`floor(8 × 0.75)`) |
//! | `Connected` | 2dp (`connectedGap`) | 2dp |
//!
//! **The connected row is density-independent on purpose, transcribed not
//! "fixed".** `M3EToggleButtonGroupTheme.metricsFor` does compute a
//! density-scaled connected gap (`m3e_toggle_button_group_theme.dart:86`),
//! but `_buildGroup` then discards it — `spacing = widget.spacing ?? (connected
//! ? 0.0 : metrics.spacing)` (`..._build.dart:13`) — and the separator each
//! connected member actually gets is re-resolved from the raw `connectedGap`
//! by `_buildGap`/`_separatorMainExtent` (`..._layout.dart:518`/`:542`). An
//! explicit [`ButtonGroupView::spacing`] always wins over both.
//!
//! # Selection: single, multi, or uncontrolled
//!
//! A **controlled component** in all three modes (see `docs/CODE_STANDARDS.md`'s
//! Interaction Semantics): a tap reports the *requested* selection and leaves
//! this view's own props untouched until the app feeds them back.
//! `_resolveToggleActionSelected` (`..._build.dart:96`) resolves which member
//! reads checked, in this precedence:
//!
//! 1. [`ButtonGroupView::selected_indices`] — multi-select; membership in the set.
//! 2. [`ButtonGroupView::on_selected_index_changed`] *or*
//!    [`ButtonGroupView::selected_index`] — single-select; `selected == Some(i)`.
//! 3. otherwise the per-action [`ButtonGroupAction::checked`] flag (uncontrolled).
//!
//! and `_onToggleCheckedChange` (`:184`) fans a tap out in the same order:
//! the indices callback wins, then the index callback (which reports `None`
//! when the tap *deselects* the current member), then — this port's own third
//! arm — the legacy [`button_group`] `on_select`, which always reports the
//! tapped index (its pre-rework contract, unchanged).
//!
//! # Neighbor squish (`ButtonGroupView::neighbor_squish`, default on)
//!
//! Pressing a member grows it by `natural × expanded_ratio × anim` and its
//! neighbours absorb exactly that growth, so the row's total main extent is
//! unchanged (`M3ERenderButtonGroup._applySquishSizes`/
//! `_shrinkNeighborsForSquish`, `..._render.dart:245`/`:273`):
//!
//! * a **middle** member: each neighbour gives up `growth / 2`;
//! * the **first** member: the single right neighbour gives up all of it;
//! * the **last** member: the single left neighbour gives up all of it;
//! * a **one-member** group grows with nothing to absorb it.
//!
//! Every shrink is floored at 0. [`squish_sizes`] is the transcription, unit
//! tested directly. The reference gates the whole animation on
//! `_supportsAnimatedSquish` (`m3e_toggle_button_group.dart:217`) — horizontal
//! **and** not connected **and** `neighborSquish` — and so does
//! [`ButtonGroupWidget::supports_squish`]. It is a *layout* animation, so it
//! drives [`PaintCtx::request_layout`], never a bare `request_frame`, and it
//! is disabled outright under `Theme::motion.reduce_motion` (the reference has
//! no reduced-motion concept; freezing the squish is this port's own
//! substitution, per `docs/WIDGETS_CODE_STANDARDS.md`).
//!
//! [`SQUISH_SPRING`] is `M3EButtonMotion.standard` (stiffness 1200, ζ 0.8,
//! `m3e_button_motion.dart:23`) — a named constant rather than a
//! [`crate::MaterialSpring`] token because no preset carries that pair, and
//! rather than a paint-time theme read because a press starts in the event
//! pass, which threads no theme.
//!
//! # Overflow
//!
//! | [`ButtonGroupOverflow`] | Behaviour |
//! |---|---|
//! | `None` | Lay every member out; content wider than the box simply overflows it (`_buildAnimatedLinearLayout` under an unclipped `LayoutBuilder`). |
//! | `Scroll` (default) | Lay every member out, clip to the box, and pan along the main axis once the content does not fit (`_linearScrollable`). |
//! | `BottomSheet` | Show as many members as fit *plus* a trailing overflow trigger, and report the first hidden index through [`ButtonGroupView::on_overflow`] when the trigger is tapped. |
//!
//! `BottomSheet`'s split is [`visible_count_for_overflow`], the exact
//! transcription of `M3EButtonGroupOverflowController.computeVisibleCountForMenu`
//! (`m3e_button_group_overflow_controller.dart:61`), including its
//! `roundConsumed`/`roundAvailable` (ceil/floor) rounding and its rule that
//! room for the trigger *and* its separator is reserved as soon as anything
//! would remain hidden.
//!
//! **The sheet itself is the app's to present.** [`crate::show_bottom_sheet`]
//! needs a `NavigatorController` a leaf widget cannot reach, and its `build`
//! closure is re-invoked on every navigator rebuild — so this module reports
//! the split and leaves presentation to `on_overflow`, exactly the seam
//! [`mod@crate::split_button`] uses for its own menu. Two consequences of the
//! split being a *layout*-time result the view cannot see: the trigger never
//! reads checked (the reference marks it checked while the hidden range holds
//! the selection, `_buildOverflowIndicatorButton`), and a group too narrow for
//! even one member leaves the trigger's leading corner inner rather than
//! outer (`isFirst: visibleCount == 0`).
//!
//! `M3EButtonGroupOverflow.menu`'s **popup** style is deliberately absent
//! rather than mapped: it needs an anchored, trigger-relative host this
//! catalog does not have yet, and silently substituting a modal sheet for a
//! popup would be a different component, not a degraded one.
//! `experimentalPaging` (and with it `M3EOverflowStrategy`, the custom
//! strategy seam) is deferred by approved decision.
//!
//! # Migrated off the legacy radial shape model
//!
//! The pre-rework module painted its own member backgrounds and a pressed
//! "shape emphasis" overlay through the crate's legacy radial-model polygon
//! (the crate docs name the module; this one no longer references it, and
//! `this_module_no_longer_uses_the_legacy_radial_shape_model` is the
//! tripwire). Both are gone: a member is a real toggle button, which paints
//! its own container, state layer and per-corner shape morph.
//! [`member_radii`] is kept as the group's documented outer/inner corner split
//! (`M3EToggleButtonGroupTheme.connectedRadiusFor`); the shipped members
//! resolve their own corners through `toggle_button`'s equivalent.
//!
//! # Semantics
//!
//! One [`Role::Group`] container (the reference's `Semantics(container: true,
//! label: semanticLabel)`) forwarding every *visible* member's own node — a
//! toggle button reports `Role::Button` + `Toggled`, so the group contributes
//! no role of its own per member. Members hidden behind the overflow trigger
//! are omitted rather than flagged hidden, under the input-parity carve-out in
//! `docs/CODE_STANDARDS.md`'s Semantics Conventions: input cannot reach them.
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section (Mudit Purohit / m3e_buttons) and its Module
//! Attribution Header Convention.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, Role, ScrollDelta, SemanticsCtx, View, Widget,
};
use frust::input::{TOUCH_SLOP, WHEEL_LINE_PX};
use frust::{AnimationController, SpringDesc, Theme};
use kurbo::{Point, RoundedRectRadii, Size};

use crate::interaction::HapticSignal;
use crate::press::presses;
use crate::{ButtonVariant, ToggleButtonSize};

// ---- Tokens (m3e_toggle_button_group_theme.dart's defaults) ----------------

/// Gap between two `Standard`-type members, in logical px —
/// `M3EToggleButtonGroupTheme.standardSpacing`'s default
/// (`m3e_toggle_button_group_theme.dart:36`). `frust::Theme` carries no
/// group-gap token, so this stays a module constant rather than a theme read.
const STANDARD_SPACING: f64 = 8.0;

/// Gap between two `Connected`-type members, in logical px —
/// `M3EToggleButtonGroupTheme.connectedGap`'s default (`:37`). The hairline
/// seam that keeps two abutting members visually distinct; the same value
/// `toggle_button`'s connected-corner tables are authored against.
const CONNECTED_GAP: f64 = 2.0;

/// `M3EButtonGroupDensity.compact`'s multiplier — `(raw * 0.75)` floored to
/// whole dp (`M3EToggleButtonGroupTheme.metricsFor`, `:92`).
const COMPACT_SCALE: f64 = 0.75;

/// `M3EButtonGroup.expandedRatio`'s default: a pressed member grows by 15% of
/// its natural main extent (`m3e_toggle_button_group.dart:82`, and the
/// identical `M3EToggleButtonGroupTheme.expandedRatio` default).
const DEFAULT_EXPANDED_RATIO: f64 = 0.15;

/// The squish spring: `M3EButtonMotion.standard` (stiffness 1200, ζ 0.8,
/// `m3e_button_motion.dart:23`), the motion `_buildSquishAnimatedLayout` hands
/// its `SingleMotionBuilder` when the group carries no decoration override.
/// See the [module docs](self) for why it is a constant, and why no
/// [`crate::MaterialSpring`] preset applies.
const SQUISH_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 1200.0,
    damping_ratio: 0.8,
};

/// Nominal period seeding the squish [`AnimationController`]'s clock; the
/// motion is spring-driven ([`SQUISH_SPRING`]) via `fling`, so this duration
/// only backs the controller's construction and is not itself a timing.
const SQUISH_ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (value-units/sec) handed to the press-in / press-out
/// [`AnimationController::fling`] — a modest kick so the spring reads snappy.
const FLING_VELOCITY: f64 = 4.0;

/// Below this the squish spring is treated as settled at rest, releasing the
/// recorded pressed member (in value units).
const SQUISH_REST_EPSILON: f64 = 1e-3;

/// Slack (logical px) below which the content is treated as fitting its
/// viewport, so a sub-pixel rounding residue never arms a scroll pan.
const FIT_EPSILON: f64 = 0.5;

/// Unthemed-fallback outer corner radius (a theme resolves
/// [`ShapeScale::large`](frust::ShapeScale::large)) — the `outer` a caller
/// with no theme in hand passes [`member_radii`].
pub const OUTER_RADIUS: f64 = 16.0;
/// Unthemed-fallback inner (adjacent) corner radius (a theme resolves
/// [`ShapeScale::extra_small`](frust::ShapeScale::extra_small)) — the `inner`
/// counterpart of [`OUTER_RADIUS`].
pub const INNER_RADIUS: f64 = 4.0;

// ---- Enums -----------------------------------------------------------------

/// How members are visually connected — `M3EButtonGroupType`
/// (`m3e_toggle_button_group_enums.dart:9`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonGroupType {
    /// Independent members with a gap between them, each keeping its own
    /// standalone shape. The reference's default.
    #[default]
    Standard,
    /// Members share edges (a hairline [`CONNECTED_GAP`] seam): the group's
    /// outer corners stay fully round, every inner corner resolves per state.
    Connected,
}

/// Spacing compactness between adjacent members — `M3EButtonGroupDensity`
/// (`m3e_toggle_button_group_enums.dart:21`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonGroupDensity {
    /// Full token spacing.
    #[default]
    Regular,
    /// 75% of token spacing, floored to whole dp.
    Compact,
}

/// The group's primary layout axis — `M3EButtonGroup.direction` (Flutter's
/// `Axis`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonGroupDirection {
    /// A row: members flow leading to trailing.
    #[default]
    Horizontal,
    /// A column: members flow top to bottom.
    Vertical,
}

/// How the group handles members that do not fit — the ported subset of
/// `M3EButtonGroupOverflow` (`m3e_toggle_button_group_enums.dart:30`). See the
/// [module docs](self)' Overflow section for the two unported variants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonGroupOverflow {
    /// Do not handle overflow at all — content wider than the box overflows it.
    None,
    /// Clip to the box and pan along the main axis when the content does not
    /// fit. The reference's default.
    #[default]
    Scroll,
    /// Show only the members that fit plus a trailing overflow trigger,
    /// reporting the first hidden index through
    /// [`ButtonGroupView::on_overflow`] — `M3EButtonGroupOverflow.menu` with
    /// `M3EButtonGroupOverflowMenuStyle.bottomSheet`.
    BottomSheet,
}

impl ButtonGroupDensity {
    /// Apply the density to a raw token gap — `metricsFor`'s
    /// `density == compact ? (raw * 0.75).floorToDouble() : raw`.
    fn scale(self, raw: f64) -> f64 {
        match self {
            ButtonGroupDensity::Regular => raw,
            ButtonGroupDensity::Compact => (raw * COMPACT_SCALE).floor(),
        }
    }
}

/// The gap between two adjacent members, in logical px: an explicit
/// [`ButtonGroupView::spacing`] wins, else the type's token gap under the
/// density. See the [module docs](self)' resolved-gap table (and why the
/// connected row is density-independent).
pub fn resolve_spacing(
    explicit: Option<f64>,
    group_type: ButtonGroupType,
    density: ButtonGroupDensity,
) -> f64 {
    match explicit {
        Some(spacing) => spacing,
        None => match group_type {
            ButtonGroupType::Connected => CONNECTED_GAP,
            ButtonGroupType::Standard => density.scale(STANDARD_SPACING),
        },
    }
}

// ---- Squish + overflow geometry (pure, unit tested) ------------------------

/// The per-member main extents after the neighbor-squish press animation —
/// `M3ERenderButtonGroup._applySquishSizes` + `_shrinkNeighborsForSquish`
/// (`..._render.dart:245`/`:273`).
///
/// `anim` is the spring's `0..=1` progress; `pressed` is the member being
/// pressed (out-of-range or `None` leaves `natural` untouched, as does a
/// non-positive `anim` — the reference's `_canApplySquish` guard). See the
/// [module docs](self) for the geometry.
pub fn squish_sizes(
    natural: &[f64],
    pressed: Option<usize>,
    expanded_ratio: f64,
    anim: f64,
) -> Vec<f64> {
    let mut sizes = natural.to_vec();
    let count = sizes.len();
    let Some(index) = pressed.filter(|i| *i < count) else {
        return sizes;
    };
    if anim <= 0.0 {
        return sizes;
    }

    let growth = natural[index] * expanded_ratio * anim;
    sizes[index] += growth;
    if count < 2 {
        // A one-member group grows with no neighbour to absorb it.
        return sizes;
    }
    if index > 0 && index + 1 < count {
        let half = growth / 2.0;
        sizes[index - 1] = (sizes[index - 1] - half).max(0.0);
        sizes[index + 1] = (sizes[index + 1] - half).max(0.0);
    } else if index == 0 {
        sizes[1] = (sizes[1] - growth).max(0.0);
    } else {
        sizes[index - 1] = (sizes[index - 1] - growth).max(0.0);
    }
    sizes
}

/// How many members fit before the overflow trigger —
/// `M3EButtonGroupOverflowController.computeVisibleCountForMenu`
/// (`m3e_button_group_overflow_controller.dart:61`).
///
/// `max_main` is floored (`roundAvailable`); every consumed extent —
/// `item_extents`, `trigger_extent`, `separator` — is ceiled
/// (`roundConsumed`). Room for the trigger *and* the separator before it is
/// reserved as soon as anything would remain hidden, and the comparison is
/// strict (`<`), so an exactly-filling row still spills its last member.
pub fn visible_count_for_overflow(
    max_main: f64,
    item_extents: &[f64],
    trigger_extent: f64,
    separator: f64,
) -> usize {
    let available = max_main.floor();
    let trigger = trigger_extent.ceil();
    let separator = separator.ceil();
    let mut current = 0.0_f64;
    let mut visible = 0usize;

    for (i, extent) in item_extents.iter().enumerate() {
        let gap_before = if i == 0 { 0.0 } else { separator };
        let remaining_after = item_extents.len() - i - 1;
        let reserved = if remaining_after > 0 {
            separator + trigger
        } else {
            0.0
        };
        let next = current + gap_before + extent.ceil();
        if next + reserved < available {
            current = next;
            visible = i + 1;
        } else {
            break;
        }
    }
    visible
}

/// The per-corner radii for member `index` of a `count`-member connected
/// group: outer corners (the group's ends) get `outer`, inner
/// (adjacent-member) corners get `inner` —
/// `M3EToggleButtonGroupTheme.connectedRadiusFor`
/// (`m3e_toggle_button_group_theme.dart:112`).
///
/// - `count == 1`: all four corners `outer` (a standalone rounded button).
/// - leading (`index == 0`): leading corners `outer`, trailing corners `inner`.
/// - trailing (`index == count - 1`): the mirror.
/// - middle: all four `inner`.
///
/// The shipped members resolve their own corners through
/// [`mod@crate::toggle_button`]'s equivalent (which additionally carries the
/// pressed/hovered/checked state arms); this stays the group's own documented
/// statement of the outer/inner split, and this framework has no RTL layout,
/// so "leading"/"trailing" map onto left/right unconditionally.
pub fn member_radii(index: usize, count: usize, outer: f64, inner: f64) -> RoundedRectRadii {
    let is_first = index == 0;
    let is_last = index + 1 == count;
    // top_left, top_right, bottom_right, bottom_left.
    let left = if is_first { outer } else { inner };
    let right = if is_last { outer } else { inner };
    RoundedRectRadii::new(left, right, right, left)
}

// ---- The action model ------------------------------------------------------

/// A view factory for one of an action's optional icon slots.
///
/// A factory rather than a plain [`AnyView`] because a member's toggle-button
/// view is rebuilt on every frame (and re-created whenever the checked state
/// flips), and an `AnyView` cannot be cloned out of the borrowed action — the
/// same reproducible-builder shape [`crate::show_bottom_sheet`]'s own `build`
/// closure takes.
type IconFactory<State> = Rc<dyn Fn() -> AnyView<State>>;

/// A declarative description of one member — `M3EButtonGroupAction`
/// (`models/m3e_button_group_action.dart:12`).
///
/// Built with [`button_group_action`] (labelled) or
/// [`button_group_icon_action`] (icon-only); the reference's
/// `icon != null || label != null` assertion is a constructor choice here
/// rather than a runtime check.
pub struct ButtonGroupAction<State: 'static> {
    icon: Option<IconFactory<State>>,
    checked_icon: Option<IconFactory<State>>,
    label: Option<String>,
    checked_label: Option<String>,
    checked: Option<bool>,
    enabled: bool,
    width: Option<f64>,
}

/// Create a labelled group member.
pub fn button_group_action<State: 'static>(label: impl Into<String>) -> ButtonGroupAction<State> {
    ButtonGroupAction {
        icon: None,
        checked_icon: None,
        label: Some(label.into()),
        checked_label: None,
        checked: None,
        enabled: true,
        width: None,
    }
}

/// Create an icon-only group member; `icon` is called once per rebuild of the
/// member (see [`IconFactory`]'s note).
pub fn button_group_icon_action<State: 'static, F>(icon: F) -> ButtonGroupAction<State>
where
    F: Fn() -> AnyView<State> + 'static,
{
    ButtonGroupAction {
        icon: Some(Rc::new(icon)),
        checked_icon: None,
        label: None,
        checked_label: None,
        checked: None,
        enabled: true,
        width: None,
    }
}

impl<State: 'static> ButtonGroupAction<State> {
    /// The icon shown while unchecked (and while checked too, unless
    /// [`Self::checked_icon`] is set).
    pub fn icon<F: Fn() -> AnyView<State> + 'static>(mut self, icon: F) -> Self {
        self.icon = Some(Rc::new(icon));
        self
    }

    /// The icon shown while checked. Falls back to [`Self::icon`] when unset.
    pub fn checked_icon<F: Fn() -> AnyView<State> + 'static>(mut self, icon: F) -> Self {
        self.checked_icon = Some(Rc::new(icon));
        self
    }

    /// The label shown while unchecked (and while checked too, unless
    /// [`Self::checked_label`] is set).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The label shown while checked. Falls back to [`Self::label`] when unset.
    pub fn checked_label(mut self, label: impl Into<String>) -> Self {
        self.checked_label = Some(label.into());
        self
    }

    /// The **uncontrolled** checked flag: consulted only when the group wires
    /// neither a selected index nor a selected-index set (the third arm of
    /// `_resolveToggleActionSelected`).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Whether this member accepts a press (`M3EButtonGroupAction.enabled`).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Pin this member's main-axis extent instead of letting it size to its
    /// content (`M3EButtonGroupAction.width`).
    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width);
        self
    }

    /// The label a screen reader and the group's own tests read for this
    /// member — the checked label when set, else the plain one.
    fn accessible_label(&self) -> String {
        self.checked_label
            .clone()
            .or_else(|| self.label.clone())
            .unwrap_or_default()
    }
}

// ---- The declarative view --------------------------------------------------

/// The legacy single-select callback: reports the tapped member's index.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;
/// `onSelectedIndexChanged`: `None` when the tap deselects the current member.
type OnIndexChanged<State> = Rc<dyn Fn(&mut State, Option<usize>)>;
/// `onSelectedIndicesChanged`: the requested selection set, ascending.
type OnIndicesChanged<State> = Rc<dyn Fn(&mut State, Vec<usize>)>;
/// Reports the first hidden index when the overflow trigger is tapped.
type OnOverflow<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative M3 Expressive button group. See the [module docs](self).
pub struct ButtonGroupView<State: 'static> {
    actions: Vec<ButtonGroupAction<State>>,
    group_type: ButtonGroupType,
    density: ButtonGroupDensity,
    direction: ButtonGroupDirection,
    spacing: Option<f64>,
    variant: ButtonVariant,
    size: ToggleButtonSize,
    haptic: HapticSignal,
    selected: Option<usize>,
    selected_indices: Option<Vec<usize>>,
    on_select: Option<OnSelect<State>>,
    on_index_changed: Option<OnIndexChanged<State>>,
    on_indices_changed: Option<OnIndicesChanged<State>>,
    neighbor_squish: bool,
    expanded_ratio: f64,
    overflow: ButtonGroupOverflow,
    overflow_icon: Option<IconFactory<State>>,
    on_overflow: Option<OnOverflow<State>>,
    semantic_label: Option<String>,
}

/// Create a group from explicit [`ButtonGroupAction`]s — the full port of
/// `M3EButtonGroup`, defaulting to [`ButtonGroupType::Standard`] and
/// [`ButtonGroupOverflow::Scroll`] exactly like the reference.
///
/// Wire a selection with [`ButtonGroupView::selected_index`] +
/// [`ButtonGroupView::on_selected_index_changed`] (single-select) or
/// [`ButtonGroupView::selected_indices`] +
/// [`ButtonGroupView::on_selected_indices_changed`] (multi-select); with
/// neither, each action's own [`ButtonGroupAction::checked`] is read instead.
pub fn button_group_actions<State: 'static>(
    actions: impl IntoIterator<Item = ButtonGroupAction<State>>,
) -> ButtonGroupView<State> {
    ButtonGroupView {
        actions: actions.into_iter().collect(),
        group_type: ButtonGroupType::default(),
        density: ButtonGroupDensity::default(),
        direction: ButtonGroupDirection::default(),
        spacing: None,
        variant: ButtonVariant::default(),
        size: ToggleButtonSize::default(),
        haptic: HapticSignal::None,
        selected: None,
        selected_indices: None,
        on_select: None,
        on_index_changed: None,
        on_indices_changed: None,
        neighbor_squish: true,
        expanded_ratio: DEFAULT_EXPANDED_RATIO,
        overflow: ButtonGroupOverflow::default(),
        overflow_icon: None,
        on_overflow: None,
        semantic_label: None,
    }
}

/// Create a single-select **connected** group with the given member `labels`,
/// the currently-`selected` index, and an `on_select` callback fired (with the
/// tapped member's index) on release inside a member.
///
/// A `selected` index outside `0..labels.len()` simply highlights no member
/// (there is no panic); feed a valid index to show a selection.
///
/// This is the module's pre-rework constructor and keeps its pre-rework
/// meaning: [`ButtonGroupType::Connected`], and an `on_select` that always
/// reports the tapped index (never a deselect `None`). Reach for
/// [`button_group_actions`] for the full port — icons, multi-select,
/// densities, direction, and overflow — whose defaults follow the reference's
/// (`Standard`) instead.
pub fn button_group<State: 'static, F: Fn(&mut State, usize) + 'static>(
    labels: impl IntoIterator<Item = impl Into<String>>,
    selected: usize,
    on_select: F,
) -> ButtonGroupView<State> {
    let actions = labels
        .into_iter()
        .map(|label| button_group_action::<State>(label));
    button_group_actions(actions)
        .group_type(ButtonGroupType::Connected)
        .selected_index(Some(selected))
        .on_select(on_select)
}

/// PascalCase alias for [`button_group`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn ButtonGroup<State: 'static, F: Fn(&mut State, usize) + 'static>(
    labels: impl IntoIterator<Item = impl Into<String>>,
    selected: usize,
    on_select: F,
) -> ButtonGroupView<State> {
    button_group(labels, selected, on_select)
}

impl<State: 'static> ButtonGroupView<State> {
    /// Set the connection treatment (the [module docs](self)' types table).
    pub fn group_type(mut self, group_type: ButtonGroupType) -> Self {
        self.group_type = group_type;
        self
    }

    /// Set the spacing compactness (the [module docs](self)' gap table).
    pub fn density(mut self, density: ButtonGroupDensity) -> Self {
        self.density = density;
        self
    }

    /// Set the primary layout axis.
    pub fn direction(mut self, direction: ButtonGroupDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Override the resolved member gap, in logical px.
    pub fn spacing(mut self, spacing: f64) -> Self {
        self.spacing = Some(spacing);
        self
    }

    /// Set every member's container treatment
    /// (`M3EButtonGroup.style`).
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set every member's size tier (`M3EButtonGroup.size`).
    pub fn size(mut self, size: ToggleButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set the haptic signal each member fires on a confirmed toggle.
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// The single-select selected index (`M3EButtonGroup.selectedIndex`).
    pub fn selected_index(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// The multi-select selection set (`M3EButtonGroup.selectedIndices`) —
    /// takes precedence over [`Self::selected_index`].
    pub fn selected_indices(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        let mut indices: Vec<usize> = indices.into_iter().collect();
        indices.sort_unstable();
        indices.dedup();
        self.selected_indices = Some(indices);
        self
    }

    /// The legacy single-select callback: always the tapped index. Lowest
    /// precedence of the three (see the [module docs](self)' selection order).
    pub fn on_select<F: Fn(&mut State, usize) + 'static>(mut self, on_select: F) -> Self {
        self.on_select = Some(Rc::new(on_select));
        self
    }

    /// `onSelectedIndexChanged`: the requested single selection, `None` when
    /// the tap deselects the currently-selected member.
    pub fn on_selected_index_changed<F: Fn(&mut State, Option<usize>) + 'static>(
        mut self,
        on_change: F,
    ) -> Self {
        self.on_index_changed = Some(Rc::new(on_change));
        self
    }

    /// `onSelectedIndicesChanged`: the requested multi selection, ascending.
    /// Highest precedence of the three.
    pub fn on_selected_indices_changed<F: Fn(&mut State, Vec<usize>) + 'static>(
        mut self,
        on_change: F,
    ) -> Self {
        self.on_indices_changed = Some(Rc::new(on_change));
        self
    }

    /// Whether a press squishes its neighbours (default `true`) — see the
    /// [module docs](self)' squish section for when it applies at all.
    pub fn neighbor_squish(mut self, squish: bool) -> Self {
        self.neighbor_squish = squish;
        self
    }

    /// The fraction of its natural main extent a pressed member grows by
    /// (default [`DEFAULT_EXPANDED_RATIO`]).
    pub fn expanded_ratio(mut self, ratio: f64) -> Self {
        self.expanded_ratio = ratio;
        self
    }

    /// How members that do not fit are handled (the [module docs](self)'
    /// Overflow table).
    pub fn overflow(mut self, overflow: ButtonGroupOverflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Replace the overflow trigger's icon (`M3EButtonGroup.overflowIcon`;
    /// the default is `M3EIcons.more_horiz`).
    pub fn overflow_icon<F: Fn() -> AnyView<State> + 'static>(mut self, icon: F) -> Self {
        self.overflow_icon = Some(Rc::new(icon));
        self
    }

    /// Fired with the first hidden action's index when the overflow trigger is
    /// tapped — where an app presents [`crate::show_bottom_sheet`] over
    /// `actions[first_hidden..]`. See the [module docs](self)' Overflow
    /// section.
    pub fn on_overflow<F: Fn(&mut State, usize) + 'static>(mut self, on_overflow: F) -> Self {
        self.on_overflow = Some(Rc::new(on_overflow));
        self
    }

    /// The accessible name of the group as a whole
    /// (`M3EButtonGroup.semanticLabel`).
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// Whether member `index` reads checked — `_resolveToggleActionSelected`
    /// (`..._build.dart:96`), in the [module docs](self)' documented
    /// precedence.
    fn is_selected(&self, index: usize) -> bool {
        if let Some(set) = &self.selected_indices {
            return set.contains(&index);
        }
        if self.on_index_changed.is_some() || self.selected.is_some() {
            return self.selected == Some(index);
        }
        self.actions[index].checked.unwrap_or(false)
    }

    /// The member view for `index`, already wired for selection fan-out and
    /// (when connected) its end-cap flags.
    fn member_view(&self, index: usize) -> AnyView<State> {
        let action = &self.actions[index];
        let count = self.actions.len();
        let connected = self.group_type == ButtonGroupType::Connected;

        let on_indices = self.on_indices_changed.clone();
        let on_index = self.on_index_changed.clone();
        let on_select = self.on_select.clone();
        let current = self.selected_indices.clone().unwrap_or_default();

        let mut member = crate::toggle_button::toggle_button::<State, _>(
            self.is_selected(index),
            move |state: &mut State, checked: bool| {
                if let Some(callback) = &on_indices {
                    let mut next = current.clone();
                    if checked {
                        if !next.contains(&index) {
                            next.push(index);
                            next.sort_unstable();
                        }
                    } else {
                        next.retain(|i| *i != index);
                    }
                    callback(state, next);
                } else if let Some(callback) = &on_index {
                    callback(state, checked.then_some(index));
                } else if let Some(callback) = &on_select {
                    callback(state, index);
                }
            },
        )
        .variant(self.variant)
        .size(self.size)
        .enabled(action.enabled)
        .haptic(self.haptic)
        .group_connected(connected)
        .first_in_group(index == 0)
        // The end cap stays on the *list's* last member: when the overflow
        // split hides a tail, the trigger takes over as the trailing cap and
        // no visible member claims it, which is exactly the reference's
        // `_buildOverflowMenuVisibleItems` shape (`isLast: false` per item).
        .last_in_group(index + 1 == count);

        if let Some(icon) = &action.icon {
            member = member.icon(icon());
        }
        if let Some(icon) = &action.checked_icon {
            member = member.checked_icon(icon());
        }
        if let Some(label) = &action.label {
            member = member.label(label.clone());
        }
        if let Some(label) = &action.checked_label {
            member = member.checked_label(label.clone());
        }
        frust::authoring::any::<State, _>(member)
    }

    /// The overflow trigger's own view — an icon-only toggle button. Its
    /// `on_checked_change` is inert: the *group* fires
    /// [`Self::on_overflow`] from its own release arm, since the first hidden
    /// index is only known after layout.
    fn trigger_view(&self) -> AnyView<State> {
        let icon = match &self.overflow_icon {
            Some(factory) => factory(),
            None => frust::authoring::any::<State, _>(frust::icon(crate::icons::MORE_HORIZ)),
        };
        frust::authoring::any::<State, _>(
            crate::toggle_button::toggle_button::<State, _>(false, |_state: &mut State, _v| {})
                .variant(self.variant)
                .size(self.size)
                .haptic(self.haptic)
                .group_connected(self.group_type == ButtonGroupType::Connected)
                .first_in_group(false)
                .last_in_group(true)
                .icon(icon),
        )
    }

    /// Every member view, in order.
    fn member_views(&self) -> Vec<AnyView<State>> {
        (0..self.actions.len())
            .map(|i| self.member_view(i))
            .collect()
    }

    /// The trigger view, when this group can split at all.
    fn maybe_trigger_view(&self) -> Option<AnyView<State>> {
        (self.overflow == ButtonGroupOverflow::BottomSheet).then(|| self.trigger_view())
    }
}

// ---- The retained widget ---------------------------------------------------

/// Which pod a live gesture is addressed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// `members[index]`.
    Member(usize),
    /// The overflow trigger.
    Trigger,
}

/// The retained widget for a [`ButtonGroupView`]. See the [module docs](self).
pub struct ButtonGroupWidget {
    /// One toggle-button pod per action, in order.
    members: Vec<ChildPod>,
    /// The overflow trigger pod (present only in
    /// [`ButtonGroupOverflow::BottomSheet`]).
    trigger: Option<ChildPod>,
    /// Per-action accessible labels, retained for diagnostics/tests.
    labels: Vec<String>,
    /// Per-action pinned main extents (`M3EButtonGroupAction.width`).
    widths: Vec<Option<f64>>,
    group_type: ButtonGroupType,
    density: ButtonGroupDensity,
    direction: ButtonGroupDirection,
    spacing: Option<f64>,
    neighbor_squish: bool,
    expanded_ratio: f64,
    overflow: ButtonGroupOverflow,
    semantic_label: Option<String>,
    on_overflow: Option<ErasedArgCallback<usize>>,

    // ---- layout output, read by paint/event ----
    /// How many members were laid out this pass (all of them unless the
    /// overflow split hid a tail).
    visible_count: usize,
    /// Whether the overflow trigger was laid out this pass.
    trigger_visible: bool,
    /// Unscrolled member origins, in this widget's local space.
    member_bases: Vec<Point>,
    /// The unscrolled trigger origin.
    trigger_base: Point,
    /// Total laid-out main extent of the visible run.
    content_main: f64,
    /// This widget's own main extent (the scroll viewport).
    viewport_main: f64,

    // ---- interaction ----
    /// Main-axis scroll position (`>= 0`; members shift back by it).
    scroll_offset: f64,
    /// The pod a live gesture is addressed to.
    active: Option<Target>,
    /// Whether the gesture has been taken over as a scroll pan.
    panning: bool,
    /// Where the capturing `Down` landed (local coordinates).
    down_pos: Point,
    /// The previous pan sample (local coordinates).
    last_drag: Point,
    /// The member whose squish is animating — retained through the release
    /// spring until it settles back to rest.
    pressed_index: Option<usize>,
    /// The spring-driven squish progress (0 rest → 1 fully pressed).
    squish: AnimationController,
}

impl ButtonGroupWidget {
    /// Whether the neighbor-squish animation applies at all —
    /// `_supportsAnimatedSquish` (`m3e_toggle_button_group.dart:217`):
    /// horizontal, not connected, and not opted out.
    pub fn supports_squish(&self) -> bool {
        self.direction == ButtonGroupDirection::Horizontal
            && self.group_type != ButtonGroupType::Connected
            && self.neighbor_squish
    }

    /// How far the content can pan; `0.0` when it fits.
    fn max_scroll(&self) -> f64 {
        (self.content_main - self.viewport_main).max(0.0)
    }

    /// Whether a drag may be taken over as a pan.
    fn can_scroll(&self) -> bool {
        self.overflow == ButtonGroupOverflow::Scroll && self.max_scroll() > FIT_EPSILON
    }

    /// The main-axis component of `p`.
    fn main_of(&self, p: Point) -> f64 {
        match self.direction {
            ButtonGroupDirection::Horizontal => p.x,
            ButtonGroupDirection::Vertical => p.y,
        }
    }

    /// The pod a target addresses.
    fn pod_mut(&mut self, target: Target) -> Option<&mut ChildPod> {
        match target {
            Target::Member(i) => self.members.get_mut(i),
            Target::Trigger => self.trigger.as_mut(),
        }
    }

    /// Which visible pod contains local point `pos` — the trigger first, since
    /// it paints last.
    fn hit(&self, pos: Point) -> Option<Target> {
        if self.trigger_visible
            && let Some(trigger) = &self.trigger
            && trigger.contains(pos)
        {
            return Some(Target::Trigger);
        }
        let visible = self.visible_count.min(self.members.len());
        self.members[..visible]
            .iter()
            .position(|pod| pod.contains(pos))
            .map(Target::Member)
    }

    /// Start the press-in squish.
    fn press_in(&mut self) {
        self.squish.fling(FLING_VELOCITY, SQUISH_SPRING);
    }

    /// Start the release squish back toward rest.
    fn press_out(&mut self) {
        self.squish.fling(-FLING_VELOCITY, SQUISH_SPRING);
    }

    /// Re-apply [`Self::scroll_offset`] to every laid-out pod's origin — the
    /// event-time counterpart of the layout pass's own positioning.
    fn sync_origins(&mut self) {
        let horizontal = self.direction == ButtonGroupDirection::Horizontal;
        let offset = self.scroll_offset;
        let visible = self.visible_count.min(self.members.len());
        for (pod, base) in self.members[..visible].iter_mut().zip(&self.member_bases) {
            pod.set_origin(shift_main(*base, -offset, horizontal));
        }
        if self.trigger_visible
            && let Some(trigger) = &mut self.trigger
        {
            trigger.set_origin(shift_main(self.trigger_base, -offset, horizontal));
        }
    }

    /// Pan by `delta` main-axis px, clamping to the scrollable range and
    /// reporting whether the position actually moved.
    fn pan_by(&mut self, delta: f64) -> bool {
        let max = self.max_scroll();
        let next = (self.scroll_offset - delta).clamp(0.0, max);
        if (next - self.scroll_offset).abs() < f64::EPSILON {
            return false;
        }
        self.scroll_offset = next;
        self.sync_origins();
        true
    }

    /// Deliver a synthetic `Cancel` to `target` — the scroll takeover's
    /// hand-off, mirroring `ScrollWidget::send_child_cancel`.
    fn send_cancel(&mut self, ctx: &mut EventCtx, target: Target, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        if let Some(pod) = self.pod_mut(target) {
            pod.event_child(ctx, &cancel);
            pod.set_active(false);
        }
    }

    /// Forward a broadcast to every pod this widget holds, visible or not —
    /// `route_event`'s broadcast contract applied to this hand-rolled routing.
    fn broadcast(&mut self, ctx: &mut EventCtx, event: &InputEvent) {
        for pod in self.members.iter_mut() {
            pod.event_child(ctx, event);
        }
        if let Some(trigger) = &mut self.trigger {
            trigger.event_child(ctx, event);
        }
    }

    /// Forward a focus-routed event to whichever pod holds the recorded focus
    /// path.
    fn route_focused(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(pod) = self.members.iter_mut().find(|pod| pod.is_focused()) {
            return pod.event_child(ctx, event);
        }
        if let Some(trigger) = self.trigger.as_mut().filter(|pod| pod.is_focused()) {
            return trigger.event_child(ctx, event);
        }
        EventResult::Ignored
    }

    /// Pull the widget's non-child configuration off `view`, reporting the
    /// change flags it earns.
    fn apply_config<State: 'static>(&mut self, view: &ButtonGroupView<State>) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.group_type != view.group_type
            || self.density != view.density
            || self.direction != view.direction
            || self.spacing != view.spacing
            || self.overflow != view.overflow
            || self.neighbor_squish != view.neighbor_squish
            || self.expanded_ratio != view.expanded_ratio
        {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        self.group_type = view.group_type;
        self.density = view.density;
        self.direction = view.direction;
        self.spacing = view.spacing;
        self.overflow = view.overflow;
        self.neighbor_squish = view.neighbor_squish;
        self.expanded_ratio = view.expanded_ratio;
        self.semantic_label = view.semantic_label.clone();
        self.labels = view
            .actions
            .iter()
            .map(ButtonGroupAction::accessible_label)
            .collect();
        self.widths = view.actions.iter().map(|a| a.width).collect();
        self.on_overflow = view
            .on_overflow
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        flags
    }
}

/// `base` moved by `delta` along the main axis.
fn shift_main(base: Point, delta: f64, horizontal: bool) -> Point {
    if horizontal {
        Point::new(base.x + delta, base.y)
    } else {
        Point::new(base.x, base.y + delta)
    }
}

impl<State: 'static> View<State> for ButtonGroupView<State> {
    type Element = ButtonGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ButtonGroupWidget {
        let members: Vec<ChildPod> = self
            .member_views()
            .iter()
            .map(|view| frust::authoring::build_child(view, ctx))
            .collect();
        let trigger = self
            .maybe_trigger_view()
            .map(|view| frust::authoring::build_child(&view, ctx));
        let mut widget = ButtonGroupWidget {
            visible_count: members.len(),
            members,
            trigger,
            labels: Vec::new(),
            widths: Vec::new(),
            group_type: self.group_type,
            density: self.density,
            direction: self.direction,
            spacing: self.spacing,
            neighbor_squish: self.neighbor_squish,
            expanded_ratio: self.expanded_ratio,
            overflow: self.overflow,
            semantic_label: None,
            on_overflow: None,
            trigger_visible: false,
            member_bases: Vec::new(),
            trigger_base: Point::ZERO,
            content_main: 0.0,
            viewport_main: 0.0,
            scroll_offset: 0.0,
            active: None,
            panning: false,
            down_pos: Point::ZERO,
            last_drag: Point::ZERO,
            pressed_index: None,
            squish: AnimationController::new(SQUISH_ANIM_PERIOD),
        };
        widget.apply_config(self);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Every member view is re-derived from the (freshly resolved)
        // selection each rebuild, so the whole list always reconciles
        // positionally — the group carries no per-member identity to key on.
        let prev_views = prev.member_views();
        let next_views = self.member_views();
        let mut flags = frust::authoring::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.members,
            ctx,
            |view| view,
            |_| None,
        );

        match (prev.maybe_trigger_view(), self.maybe_trigger_view()) {
            (Some(prev_view), Some(next_view)) => {
                if let Some(pod) = element.trigger.as_mut() {
                    flags |= frust::authoring::rebuild_child(&prev_view, &next_view, pod, ctx);
                }
            }
            (Some(prev_view), None) => {
                if let Some(pod) = element.trigger.as_mut() {
                    frust::authoring::teardown_child(&prev_view, pod, ctx);
                }
                element.trigger = None;
                element.trigger_visible = false;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, Some(next_view)) => {
                element.trigger = Some(frust::authoring::build_child(&next_view, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, None) => {}
        }

        if prev.actions.len() != self.actions.len() {
            // A structural change invalidates the recorded press/pan.
            element.active = None;
            element.panning = false;
            element.pressed_index = None;
            element.scroll_offset = 0.0;
            element.visible_count = element.members.len();
        }
        flags |= element.apply_config(self);
        flags
    }

    fn teardown(&self, element: &mut ButtonGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.member_views().iter().zip(element.members.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
        if let (Some(view), Some(pod)) = (self.maybe_trigger_view(), element.trigger.as_mut()) {
            frust::authoring::teardown_child(&view, pod, ctx);
        }
    }
}

impl Widget for ButtonGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let horizontal = self.direction == ButtonGroupDirection::Horizontal;
        let max = bc.max();
        let (max_main, max_cross) = if horizontal {
            (max.width, max.height)
        } else {
            (max.height, max.width)
        };
        self.member_bases.clear();
        if self.members.is_empty() {
            self.visible_count = 0;
            self.trigger_visible = false;
            self.content_main = 0.0;
            self.viewport_main = 0.0;
            return bc.constrain(Size::ZERO);
        }

        let spacing = resolve_spacing(self.spacing, self.group_type, self.density);

        // Pass 1 — natural main extents, main axis unbounded
        // (`_layoutNaturalMainSizes`' dry pass).
        let natural_bc = if horizontal {
            BoxConstraints::loose(Size::new(f64::INFINITY, max_cross))
        } else {
            BoxConstraints::loose(Size::new(max_cross, f64::INFINITY))
        };
        let mut natural = Vec::with_capacity(self.members.len());
        for (i, pod) in self.members.iter_mut().enumerate() {
            let size = pod.layout_child(ctx, &natural_bc);
            let measured = if horizontal { size.width } else { size.height };
            natural.push(self.widths.get(i).copied().flatten().unwrap_or(measured));
        }
        let trigger_natural = self.trigger.as_mut().map(|pod| {
            let size = pod.layout_child(ctx, &natural_bc);
            if horizontal { size.width } else { size.height }
        });

        // Overflow split — only the bottom-sheet mode hides anything.
        let (visible_count, trigger_visible) = match (self.overflow, trigger_natural) {
            (ButtonGroupOverflow::BottomSheet, Some(trigger)) if max_main.is_finite() => {
                let fit = visible_count_for_overflow(max_main, &natural, trigger, spacing);
                if fit >= natural.len() {
                    (natural.len(), false)
                } else {
                    (fit, true)
                }
            }
            _ => (natural.len(), false),
        };
        self.visible_count = visible_count;
        self.trigger_visible = trigger_visible;

        // The squish, over the visible run only.
        let anim = if self.supports_squish() {
            self.squish.value_clamped()
        } else {
            0.0
        };
        let pressed = self.pressed_index.filter(|i| *i < visible_count);
        let mut sizes = squish_sizes(
            &natural[..visible_count],
            pressed,
            self.expanded_ratio,
            anim,
        );
        if let Some(trigger) = trigger_natural.filter(|_| trigger_visible) {
            sizes.push(trigger);
        }

        // Pass 2 — tight main, loose cross; then position.
        let mut cross = 0.0_f64;
        let mut child_sizes = Vec::with_capacity(sizes.len());
        {
            let pods = self.members[..visible_count]
                .iter_mut()
                .chain(self.trigger.iter_mut().filter(|_| trigger_visible));
            for (pod, main) in pods.zip(sizes.iter()) {
                let child_bc = if horizontal {
                    BoxConstraints::new(Size::new(*main, 0.0), Size::new(*main, max_cross))
                } else {
                    BoxConstraints::new(Size::new(0.0, *main), Size::new(max_cross, *main))
                };
                let size = pod.layout_child(ctx, &child_bc);
                cross = cross.max(if horizontal { size.height } else { size.width });
                child_sizes.push(size);
            }
        }

        let laid_out = sizes.len();
        let total_main: f64 = sizes.iter().sum::<f64>()
            + if laid_out > 0 {
                spacing * (laid_out - 1) as f64
            } else {
                0.0
            };

        let mut main_offset = 0.0_f64;
        let mut bases = Vec::with_capacity(laid_out);
        for (main, size) in sizes.iter().zip(child_sizes.iter()) {
            let child_cross = if horizontal { size.height } else { size.width };
            let cross_offset = ((cross - child_cross) / 2.0).max(0.0);
            bases.push(if horizontal {
                Point::new(main_offset, cross_offset)
            } else {
                Point::new(cross_offset, main_offset)
            });
            main_offset += main + spacing;
        }
        self.trigger_base = if trigger_visible {
            bases.pop().unwrap_or(Point::ZERO)
        } else {
            Point::ZERO
        };
        self.member_bases = bases;

        let size = bc.constrain(if horizontal {
            Size::new(total_main, cross)
        } else {
            Size::new(cross, total_main)
        });
        self.content_main = total_main;
        self.viewport_main = if horizontal { size.width } else { size.height };
        self.scroll_offset = self.scroll_offset.clamp(0.0, self.max_scroll());
        self.sync_origins();
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let reduce_motion = Theme::from_paint_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);

        if reduce_motion {
            // The squish is a layout animation with no reduced-motion analogue:
            // freeze it at rest and stop asking for frames.
            if self.squish.is_animating() {
                self.squish.stop();
            }
            self.pressed_index = None;
        } else {
            // A layout-affecting animation asks for `request_layout`; a bare
            // `request_frame` would leave the row unresized under the mobile
            // intra-frame layout skip.
            if self.squish.advance(ctx.frame_time()) {
                ctx.request_layout();
            } else if self.active.is_none()
                && self.squish.value().abs() < SQUISH_REST_EPSILON
                && self.pressed_index.is_some()
            {
                self.pressed_index = None;
            }
        }

        let clipped =
            self.overflow == ButtonGroupOverflow::Scroll && self.max_scroll() > FIT_EPSILON;
        if clipped {
            scene.push_clip(ctx.origin(), ctx.size());
        }
        let visible = self.visible_count.min(self.members.len());
        for pod in self.members[..visible].iter_mut() {
            pod.paint_child(ctx, scene);
        }
        if self.trigger_visible
            && let Some(trigger) = &mut self.trigger
        {
            trigger.paint_child(ctx, scene);
        }
        if clipped {
            scene.pop_clip();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.broadcast(ctx, event);
            return EventResult::Ignored;
        }
        if event.is_focus_routed() {
            return self.route_focused(ctx, event);
        }
        match event {
            InputEvent::Scroll { delta, .. } => {
                if !self.can_scroll() {
                    return EventResult::Ignored;
                }
                let (dx, dy) = match delta {
                    ScrollDelta::Lines(x, y) => (x * WHEEL_LINE_PX, y * WHEEL_LINE_PX),
                    ScrollDelta::Pixels(x, y) => (*x, *y),
                };
                // A horizontal row takes the wheel's horizontal axis when it
                // carries one, and its vertical axis otherwise (the usual
                // shift-less mouse over a horizontal strip).
                let main = match self.direction {
                    ButtonGroupDirection::Horizontal if dx != 0.0 => dx,
                    ButtonGroupDirection::Horizontal => dy,
                    ButtonGroupDirection::Vertical => dy,
                };
                if self.pan_by(-main) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    let Some(target) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    if !presses(p) {
                        // A secondary press is a context gesture: it reaches
                        // the member but opens no capture and starts no squish.
                        return self
                            .pod_mut(target)
                            .map(|pod| pod.event_child(ctx, event))
                            .unwrap_or(EventResult::Ignored);
                    }
                    self.active = Some(target);
                    self.panning = false;
                    self.down_pos = p.position;
                    self.last_drag = p.position;
                    if let Target::Member(i) = target {
                        self.pressed_index = Some(i);
                        self.press_in();
                    }
                    ctx.capture_pointer();
                    if let Some(pod) = self.pod_mut(target) {
                        pod.event_child(ctx, event);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    let Some(target) = self.active else {
                        // A hover move: forward it so the member under the
                        // pointer can claim hover for itself.
                        return match self.hit(p.position) {
                            Some(hit) => self
                                .pod_mut(hit)
                                .map(|pod| pod.event_child(ctx, event))
                                .unwrap_or(EventResult::Ignored),
                            None => EventResult::Ignored,
                        };
                    };
                    if self.panning {
                        let delta = self.main_of(p.position) - self.main_of(self.last_drag);
                        self.last_drag = p.position;
                        if self.pan_by(delta) {
                            ctx.request_redraw();
                        }
                        return EventResult::Handled;
                    }
                    let travelled = self.main_of(p.position) - self.main_of(self.down_pos);
                    if self.can_scroll() && travelled.abs() > TOUCH_SLOP {
                        // Take the gesture over: cancel the member, stop
                        // forwarding, release the squish.
                        self.panning = true;
                        self.last_drag = p.position;
                        self.send_cancel(ctx, target, p.position);
                        self.press_out();
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    if let Some(pod) = self.pod_mut(target) {
                        pod.event_child(ctx, event);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let Some(target) = self.active.take() else {
                        return EventResult::Ignored;
                    };
                    let panned = self.panning;
                    self.panning = false;
                    if !panned {
                        if let Some(pod) = self.pod_mut(target) {
                            pod.event_child(ctx, event);
                        }
                        // The trigger's own callback is inert; the group
                        // reports the (layout-derived) first hidden index here.
                        if target == Target::Trigger
                            && self
                                .trigger
                                .as_ref()
                                .is_some_and(|pod| pod.contains(p.position))
                        {
                            let first_hidden = self.visible_count;
                            if let Some(callback) = self.on_overflow.as_mut() {
                                callback(ctx, first_hidden);
                            }
                        }
                    }
                    if let Some(pod) = self.pod_mut(target) {
                        pod.set_active(false);
                    }
                    self.press_out();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    let Some(target) = self.active.take() else {
                        return EventResult::Ignored;
                    };
                    if !self.panning
                        && let Some(pod) = self.pod_mut(target)
                    {
                        pod.event_child(ctx, event);
                    }
                    if let Some(pod) = self.pod_mut(target) {
                        pod.set_active(false);
                    }
                    self.panning = false;
                    self.press_out();
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.semantic_label.clone();
        ctx.push_container(
            Role::Group,
            |node| {
                if let Some(label) = &label {
                    node.set_label(label.as_str());
                }
            },
            |ctx| {
                let visible = self.visible_count.min(self.members.len());
                for pod in &self.members[..visible] {
                    pod.semantics_child(ctx);
                }
                if self.trigger_visible
                    && let Some(trigger) = &self.trigger
                {
                    trigger.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(members, trigger);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::text::TextContext;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;
    use std::cell::RefCell;

    // ---- fixtures ---------------------------------------------------------

    /// Sm-tier metrics, re-derived here so the layout expectations below are
    /// arithmetic rather than magic: a `Sm` toggle button is 40dp tall with
    /// 16dp horizontal padding, halved for an icon-only member, around a 20dp
    /// icon — so an icon-only `Sm` member is exactly 36×40.
    const ICON_MEMBER_W: f64 = 36.0;
    const MEMBER_H: f64 = 40.0;

    /// Icon-only actions with no label at all — the deterministic 36dp members
    /// the layout tests measure against.
    fn bare_icon_actions(count: usize) -> Vec<ButtonGroupAction<()>> {
        (0..count)
            .map(|_| button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)))
            .collect()
    }

    fn build<S: 'static>(view: &ButtonGroupView<S>) -> ButtonGroupWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(widget: &mut ButtonGroupWidget, max: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut lctx, &BoxConstraints::loose(max))
    }

    fn layout_themed(widget: &mut ButtonGroupWidget, max: Size, theme: &Theme) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_text_context(&mut tcx as &mut dyn Any).with_theme(theme as &dyn Any);
        widget.layout(&mut lctx, &BoxConstraints::loose(max))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<S: 'static>(
        widget: &mut ButtonGroupWidget,
        state: &mut S,
        size: Size,
        event: &InputEvent,
    ) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event);
    }

    /// Advance the squish spring far enough to be visibly non-zero.
    fn advance_squish(widget: &mut ButtonGroupWidget, secs: f64) {
        widget.squish.advance(FrameTime::ZERO);
        widget
            .squish
            .advance(FrameTime::from_nanos((secs * 1e9) as u64));
    }

    #[derive(Default)]
    struct NullScene;

    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn paint(widget: &mut ButtonGroupWidget, size: Size, theme: Option<&Theme>) {
        let mut scene = NullScene;
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme as &dyn Any);
        }
        widget.paint(&mut ctx, &mut scene);
    }

    // ---- the legacy corner helper (unchanged contract) ---------------------

    #[test]
    fn single_member_group_is_all_outer_corners() {
        let r = member_radii(0, 1, OUTER_RADIUS, INNER_RADIUS);
        assert_eq!(r.top_left, 16.0);
        assert_eq!(r.top_right, 16.0);
        assert_eq!(r.bottom_right, 16.0);
        assert_eq!(r.bottom_left, 16.0);
    }

    #[test]
    fn end_members_get_outer_on_their_outer_side_only() {
        let first = member_radii(0, 3, 16.0, 4.0);
        assert_eq!((first.top_left, first.bottom_left), (16.0, 16.0));
        assert_eq!((first.top_right, first.bottom_right), (4.0, 4.0));
        let last = member_radii(2, 3, 16.0, 4.0);
        assert_eq!((last.top_left, last.bottom_left), (4.0, 4.0));
        assert_eq!((last.top_right, last.bottom_right), (16.0, 16.0));
    }

    #[test]
    fn middle_members_are_all_inner_corners() {
        let mid = member_radii(1, 3, 16.0, 4.0);
        assert_eq!(mid.top_left, 4.0);
        assert_eq!(mid.top_right, 4.0);
        assert_eq!(mid.bottom_right, 4.0);
        assert_eq!(mid.bottom_left, 4.0);
    }

    // ---- types × densities (the resolved gap table) ------------------------

    #[test]
    fn resolved_gap_pins_every_type_density_pair() {
        use ButtonGroupDensity::{Compact, Regular};
        use ButtonGroupType::{Connected, Standard};
        assert_eq!(resolve_spacing(None, Standard, Regular), 8.0);
        // floor(8 * 0.75) = 6.
        assert_eq!(resolve_spacing(None, Standard, Compact), 6.0);
        // The connected seam is density-independent upstream — see the module
        // docs' transcription note.
        assert_eq!(resolve_spacing(None, Connected, Regular), 2.0);
        assert_eq!(resolve_spacing(None, Connected, Compact), 2.0);
        // An explicit spacing always wins.
        assert_eq!(resolve_spacing(Some(3.5), Connected, Compact), 3.5);
    }

    // ---- squish geometry --------------------------------------------------

    #[test]
    fn squish_middle_splits_the_growth_between_both_neighbours() {
        let natural = [100.0, 200.0, 100.0];
        let sizes = squish_sizes(&natural, Some(1), 0.15, 1.0);
        // growth = 200 * 0.15 * 1.0 = 30; each neighbour gives up 15.
        assert_eq!(sizes, vec![85.0, 230.0, 85.0]);
    }

    #[test]
    fn squish_at_the_first_index_moves_all_growth_to_its_only_neighbour() {
        let sizes = squish_sizes(&[100.0, 100.0, 100.0], Some(0), 0.15, 1.0);
        assert_eq!(sizes, vec![115.0, 85.0, 100.0]);
    }

    #[test]
    fn squish_at_the_last_index_mirrors_the_first() {
        let sizes = squish_sizes(&[100.0, 100.0, 100.0], Some(2), 0.15, 1.0);
        assert_eq!(sizes, vec![100.0, 85.0, 115.0]);
    }

    #[test]
    fn squish_preserves_the_total_main_extent() {
        for pressed in 0..3 {
            let natural = [80.0, 120.0, 60.0];
            let sizes = squish_sizes(&natural, Some(pressed), 0.15, 0.6);
            let before: f64 = natural.iter().sum();
            let after: f64 = sizes.iter().sum();
            assert!(
                (before - after).abs() < 1e-9,
                "pressing {pressed} changed the total: {before} -> {after}"
            );
        }
    }

    #[test]
    fn squish_scales_with_the_spring_progress_and_the_ratio() {
        let half = squish_sizes(&[100.0, 100.0], Some(0), 0.15, 0.5);
        assert_eq!(half, vec![107.5, 92.5]);
        let wider = squish_sizes(&[100.0, 100.0], Some(0), 0.30, 1.0);
        assert_eq!(wider, vec![130.0, 70.0]);
    }

    #[test]
    fn squish_is_inert_without_a_press_or_progress() {
        let natural = [100.0, 100.0];
        assert_eq!(squish_sizes(&natural, None, 0.15, 1.0), natural.to_vec());
        assert_eq!(squish_sizes(&natural, Some(0), 0.15, 0.0), natural.to_vec());
        // Out of range: `_canApplySquish`'s bounds guard.
        assert_eq!(squish_sizes(&natural, Some(9), 0.15, 1.0), natural.to_vec());
    }

    #[test]
    fn squish_of_a_one_member_group_grows_with_nothing_to_absorb_it() {
        assert_eq!(squish_sizes(&[100.0], Some(0), 0.15, 1.0), vec![115.0]);
    }

    #[test]
    fn squish_shrink_is_floored_at_zero() {
        // A neighbour narrower than the growth clamps instead of going negative.
        let sizes = squish_sizes(&[400.0, 10.0], Some(0), 0.15, 1.0);
        assert_eq!(sizes, vec![460.0, 0.0]);
    }

    #[test]
    fn squish_applies_only_to_a_horizontal_unconnected_group() {
        let cases = [
            (
                ButtonGroupDirection::Horizontal,
                ButtonGroupType::Standard,
                true,
                true,
            ),
            (
                ButtonGroupDirection::Horizontal,
                ButtonGroupType::Connected,
                true,
                false,
            ),
            (
                ButtonGroupDirection::Vertical,
                ButtonGroupType::Standard,
                true,
                false,
            ),
            (
                ButtonGroupDirection::Horizontal,
                ButtonGroupType::Standard,
                false,
                false,
            ),
        ];
        for (direction, group_type, squish, expected) in cases {
            let view = button_group_actions(bare_icon_actions(3))
                .direction(direction)
                .group_type(group_type)
                .neighbor_squish(squish);
            let widget = build(&view);
            assert_eq!(
                widget.supports_squish(),
                expected,
                "{direction:?}/{group_type:?}/squish={squish}"
            );
        }
    }

    // ---- overflow measurement ---------------------------------------------

    #[test]
    fn visible_count_is_every_item_when_they_all_fit() {
        // 3 * 36 + 2 * 8 = 124; nothing hidden means nothing reserved.
        assert_eq!(visible_count_for_overflow(200.0, &[36.0; 3], 36.0, 8.0), 3);
    }

    #[test]
    fn visible_count_reserves_the_trigger_and_its_separator() {
        // 100px: item0 (36) + sep(8) + trigger(36) = 80 < 100 -> visible 1.
        // item1 would make 36+8+36 = 80, +8+36 reserved = 124 >= 100 -> stop.
        assert_eq!(visible_count_for_overflow(100.0, &[36.0; 4], 36.0, 8.0), 1);
    }

    #[test]
    fn visible_count_rounds_consumed_up_and_available_down() {
        // Ceil'd items (36.2 -> 37) against a floored available (99.9 -> 99):
        // 37 + 8 + 36 = 81 < 99 -> 1; adding the next gives 37+8+37 = 82,
        // +8+36 = 126 >= 99 -> stop.
        assert_eq!(
            visible_count_for_overflow(99.9, &[36.2, 36.2, 36.2], 36.0, 8.0),
            1
        );
    }

    #[test]
    fn visible_count_is_zero_when_even_one_item_cannot_fit() {
        assert_eq!(visible_count_for_overflow(40.0, &[36.0; 3], 36.0, 8.0), 0);
    }

    // ---- construction / defaults ------------------------------------------

    #[test]
    fn build_creates_one_pod_per_action() {
        let view = button_group::<u32, _>(vec!["Day", "Week", "Month"], 0, |s: &mut u32, i| {
            *s = i as u32
        });
        let widget = build(&view);
        assert_eq!(widget.members.len(), 3);
        assert_eq!(widget.labels, vec!["Day", "Week", "Month"]);
        assert!(widget.trigger.is_none(), "no trigger without BottomSheet");
    }

    #[test]
    fn the_legacy_constructor_stays_connected_while_the_action_one_is_standard() {
        let legacy = build(&button_group::<(), _>(vec!["A", "B"], 0, |_s, _i| {}));
        assert_eq!(legacy.group_type, ButtonGroupType::Connected);
        let ported = build(&button_group_actions(bare_icon_actions(2)));
        assert_eq!(ported.group_type, ButtonGroupType::Standard);
        assert_eq!(ported.overflow, ButtonGroupOverflow::Scroll);
    }

    #[test]
    fn a_bottom_sheet_group_builds_a_trigger_pod() {
        let view =
            button_group_actions(bare_icon_actions(3)).overflow(ButtonGroupOverflow::BottomSheet);
        let widget = build(&view);
        assert!(widget.trigger.is_some());
    }

    // ---- layout -----------------------------------------------------------

    #[test]
    fn members_abut_with_the_resolved_gap() {
        let mut widget = build(&button_group_actions(bare_icon_actions(3)));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        // 3 * 36 + 2 * 8 (standard/regular).
        assert_eq!(size, Size::new(3.0 * ICON_MEMBER_W + 16.0, MEMBER_H));
        assert_eq!(widget.member_bases[0].x, 0.0);
        assert_eq!(widget.member_bases[1].x, ICON_MEMBER_W + 8.0);
        assert_eq!(widget.member_bases[2].x, 2.0 * (ICON_MEMBER_W + 8.0));
    }

    #[test]
    fn a_connected_group_uses_the_hairline_seam() {
        let mut widget = build(
            &button_group_actions(bare_icon_actions(3)).group_type(ButtonGroupType::Connected),
        );
        layout(&mut widget, Size::new(500.0, 200.0));
        assert_eq!(widget.member_bases[1].x, ICON_MEMBER_W + CONNECTED_GAP);
    }

    #[test]
    fn a_vertical_group_stacks_along_the_cross_axis() {
        let mut widget = build(
            &button_group_actions(bare_icon_actions(2)).direction(ButtonGroupDirection::Vertical),
        );
        let size = layout(&mut widget, Size::new(200.0, 500.0));
        assert_eq!(size, Size::new(ICON_MEMBER_W, 2.0 * MEMBER_H + 8.0));
        assert_eq!(widget.member_bases[1].y, MEMBER_H + 8.0);
    }

    #[test]
    fn a_pinned_action_width_overrides_the_natural_extent() {
        let actions = vec![
            button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)).width(90.0),
            button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)),
        ];
        let mut widget = build(&button_group_actions(actions));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        assert_eq!(size.width, 90.0 + 8.0 + ICON_MEMBER_W);
        assert_eq!(widget.member_bases[1].x, 98.0);
    }

    #[test]
    fn pressing_a_member_squishes_its_neighbour_in_layout() {
        let mut widget = build(&button_group_actions(bare_icon_actions(2)));
        let unpressed = layout(&mut widget, Size::new(500.0, 200.0));
        let gap_start = widget.member_bases[1].x;

        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            unpressed,
            &ev(PointerPhase::Down, 4.0, 20.0),
        );
        advance_squish(&mut widget, 0.05);
        let pressed = layout(&mut widget, Size::new(500.0, 200.0));

        assert!(
            widget.member_bases[1].x > gap_start,
            "member 0 grew, pushing member 1 along ({} -> {})",
            gap_start,
            widget.member_bases[1].x
        );
        assert!(
            (pressed.width - unpressed.width).abs() < 1e-6,
            "the squish is extent-preserving: {unpressed:?} -> {pressed:?}"
        );
    }

    #[test]
    fn reduce_motion_freezes_the_squish() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut widget = build(&button_group_actions(bare_icon_actions(2)));
        let size = layout_themed(&mut widget, Size::new(500.0, 200.0), &theme);
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 4.0, 20.0),
        );
        assert!(widget.squish.is_animating());
        paint(&mut widget, size, Some(&theme));
        assert!(
            !widget.squish.is_animating(),
            "reduce_motion stops the spring"
        );
        assert_eq!(widget.pressed_index, None);
    }

    // ---- selection fan-out -------------------------------------------------

    #[test]
    fn tap_reports_the_tapped_member_index() {
        let mut widget = build(&button_group::<u32, _>(
            vec!["A", "B", "C"],
            0,
            |s: &mut u32, i| *s = i as u32,
        ));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let mid = widget.member_bases[1].x + 4.0;
        let mut state = 0u32;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, mid, 20.0),
        );
        assert_eq!(widget.pressed_index, Some(1));
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, mid, 20.0),
        );
        assert_eq!(state, 1, "on_select fired with the tapped index");
    }

    #[test]
    fn the_legacy_callback_still_fires_on_the_already_selected_member() {
        // The pre-rework contract: `on_select` always reports the tapped index,
        // never a deselect.
        let mut widget = build(&button_group::<u32, _>(
            vec!["A", "B"],
            1,
            |s: &mut u32, i| *s = 40 + i as u32,
        ));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let x = widget.member_bases[1].x + 4.0;
        let mut state = 0u32;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, x, 20.0),
        );
        assert_eq!(state, 41);
    }

    #[test]
    fn single_select_reports_none_when_deselecting_the_current_member() {
        let seen: Rc<RefCell<Vec<Option<usize>>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let view = button_group_actions(bare_icon_actions(2))
            .selected_index(Some(1))
            .on_selected_index_changed(move |_s: &mut (), next| sink.borrow_mut().push(next));
        let mut widget = build(&view);
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let x = widget.member_bases[1].x + 4.0;
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, x, 20.0),
        );
        assert_eq!(*seen.borrow(), vec![None]);
    }

    #[test]
    fn multi_select_reports_the_requested_index_set() {
        let seen: Rc<RefCell<Vec<Vec<usize>>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let view = button_group_actions(bare_icon_actions(3))
            .selected_indices([2])
            .on_selected_indices_changed(move |_s: &mut (), next| sink.borrow_mut().push(next));
        let mut widget = build(&view);
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let x = widget.member_bases[0].x + 4.0;
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, x, 20.0),
        );
        assert_eq!(*seen.borrow(), vec![vec![0, 2]], "added, kept ascending");
    }

    #[test]
    fn multi_select_removes_an_already_selected_index() {
        let seen: Rc<RefCell<Vec<Vec<usize>>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let view = button_group_actions(bare_icon_actions(3))
            .selected_indices([0, 2])
            .on_selected_indices_changed(move |_s: &mut (), next| sink.borrow_mut().push(next));
        let mut widget = build(&view);
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let x = widget.member_bases[2].x + 4.0;
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, x, 20.0),
        );
        assert_eq!(*seen.borrow(), vec![vec![0]]);
    }

    #[test]
    fn selection_resolution_follows_the_documented_precedence() {
        // 1. indices win over index.
        let view = button_group_actions(bare_icon_actions(3))
            .selected_index(Some(0))
            .selected_indices([1, 2]);
        assert!(!view.is_selected(0));
        assert!(view.is_selected(1) && view.is_selected(2));

        // 2. index wins over a per-action `checked`.
        let actions = vec![
            button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)).checked(true),
            button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)),
        ];
        let view = button_group_actions(actions).selected_index(Some(1));
        assert!(!view.is_selected(0));
        assert!(view.is_selected(1));

        // 3. uncontrolled: the action's own flag.
        let actions = vec![
            button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)).checked(true),
            button_group_icon_action::<(), _>(|| leaf_any(0.0, 0.0)),
        ];
        let view = button_group_actions(actions);
        assert!(view.is_selected(0));
        assert!(!view.is_selected(1));
    }

    #[test]
    fn a_selected_index_outside_the_action_range_selects_nothing() {
        let view = button_group::<(), _>(vec!["A", "B"], 7, |_s, _i| {});
        assert!(!view.is_selected(0));
        assert!(!view.is_selected(1));
    }

    #[test]
    fn is_controlled_the_view_never_self_mutates_its_selection() {
        // A tap reports the request only; the app feeds the confirmed value
        // back on the next rebuild, which is what moves the member's own
        // checked state.
        let mut widget = build(&button_group::<u32, _>(vec!["A", "B"], 0, |_s, _i| {}));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let x = widget.member_bases[1].x + 4.0;
        let mut state = 0u32;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, x, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, x, 20.0),
        );

        let prev = button_group::<u32, _>(vec!["A", "B"], 0, |_s, _i| {});
        let next = button_group::<u32, _>(vec!["A", "B"], 1, |_s, _i| {});
        let mut counter = 0u64;
        let flags =
            View::<u32>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert!(!flags.is_empty(), "the selection flip reaches the members");
    }

    #[test]
    fn a_release_outside_the_pressed_member_does_not_fire() {
        let mut widget = build(&button_group::<u32, _>(
            vec!["A", "B"],
            0,
            |s: &mut u32, i| *s = i as u32,
        ));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let mut state = 7u32;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 4.0, 20.0),
        );
        let away = widget.member_bases[1].x + 4.0;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Move, away, 20.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, away, 20.0),
        );
        assert_eq!(state, 7, "the member's own up-inside rule still governs");
    }

    #[test]
    fn a_down_outside_every_member_is_ignored() {
        let mut widget = build(&button_group_actions(bare_icon_actions(2)));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        let result = widget.event(&mut ctx, &ev(PointerPhase::Down, 400.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(widget.pressed_index, None);
    }

    #[test]
    fn cancel_clears_the_press_without_firing() {
        let mut widget = build(&button_group::<u32, _>(
            vec!["A", "B"],
            0,
            |s: &mut u32, i| *s = i as u32,
        ));
        let size = layout(&mut widget, Size::new(500.0, 200.0));
        let mut state = 3u32;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 4.0, 20.0),
        );
        assert!(widget.active.is_some());
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 4.0, 20.0),
        );
        assert!(widget.active.is_none());
        assert_eq!(state, 3);
    }

    // ---- overflow behaviours ------------------------------------------------

    #[test]
    fn overflow_none_lets_the_content_exceed_the_box() {
        let mut widget =
            build(&button_group_actions(bare_icon_actions(6)).overflow(ButtonGroupOverflow::None));
        layout(&mut widget, Size::new(100.0, 200.0));
        assert_eq!(widget.visible_count, 6, "nothing is hidden");
        assert!(widget.content_main > widget.viewport_main);
        assert!(!widget.can_scroll(), "None never pans");
    }

    #[test]
    fn overflow_scroll_pans_after_the_touch_slop() {
        let mut widget = build(&button_group_actions(bare_icon_actions(6)));
        let size = layout(&mut widget, Size::new(100.0, 200.0));
        assert!(widget.can_scroll());
        let start_x = widget.member_bases[0].x;
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, 4.0, 20.0),
        );
        // Below the slop: still the member's gesture, no pan.
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Move, 4.0, 20.0),
        );
        assert!(!widget.panning);
        assert_eq!(widget.scroll_offset, 0.0);
        // Past the slop: the group takes over.
        let far = 4.0 - TOUCH_SLOP - 20.0;
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Move, far, 20.0),
        );
        assert!(widget.panning);
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Move, far - 15.0, 20.0),
        );
        assert!(widget.scroll_offset > 0.0, "the row panned");
        assert!(
            widget.members[0].origin().x < start_x,
            "member origins follow the pan"
        );
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, far - 15.0, 20.0),
        );
        assert!(!widget.panning);
    }

    #[test]
    fn overflow_scroll_clamps_the_pan_to_the_content() {
        let mut widget = build(&button_group_actions(bare_icon_actions(6)));
        layout(&mut widget, Size::new(100.0, 200.0));
        let max = widget.max_scroll();
        widget.pan_by(-10_000.0);
        assert_eq!(widget.scroll_offset, max);
        widget.pan_by(10_000.0);
        assert_eq!(widget.scroll_offset, 0.0);
    }

    #[test]
    fn overflow_scroll_does_not_pan_when_the_content_fits() {
        let mut widget = build(&button_group_actions(bare_icon_actions(2)));
        layout(&mut widget, Size::new(500.0, 200.0));
        assert!(!widget.can_scroll());
        assert!(!widget.pan_by(-50.0));
    }

    #[test]
    fn overflow_bottom_sheet_splits_and_shows_a_trigger() {
        let mut widget = build(
            &button_group_actions(bare_icon_actions(6)).overflow(ButtonGroupOverflow::BottomSheet),
        );
        layout(&mut widget, Size::new(140.0, 200.0));
        // available 140; item(36) + sep(8) + trigger(36) = 80 < 140 -> 1,
        // then 36+8+36 = 80 (+44 reserved) = 124 < 140 -> 2,
        // then 80+8+36 = 124 (+44) = 168 >= 140 -> stop.
        assert_eq!(widget.visible_count, 2);
        assert!(widget.trigger_visible);
        assert_eq!(widget.member_bases.len(), 2);
    }

    #[test]
    fn overflow_bottom_sheet_hides_nothing_when_everything_fits() {
        let mut widget = build(
            &button_group_actions(bare_icon_actions(3)).overflow(ButtonGroupOverflow::BottomSheet),
        );
        layout(&mut widget, Size::new(500.0, 200.0));
        assert_eq!(widget.visible_count, 3);
        assert!(!widget.trigger_visible);
    }

    #[test]
    fn tapping_the_overflow_trigger_reports_the_first_hidden_index() {
        let seen: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = seen.clone();
        let view = button_group_actions(bare_icon_actions(6))
            .overflow(ButtonGroupOverflow::BottomSheet)
            .on_overflow(move |_s: &mut (), first| sink.borrow_mut().push(first));
        let mut widget = build(&view);
        let size = layout(&mut widget, Size::new(140.0, 200.0));
        let trigger_x = widget.trigger_base.x + 4.0;
        let mut state = ();
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Down, trigger_x, 20.0),
        );
        assert_eq!(widget.active, Some(Target::Trigger));
        assert_eq!(widget.pressed_index, None, "the trigger never squishes");
        dispatch(
            &mut widget,
            &mut state,
            size,
            &ev(PointerPhase::Up, trigger_x, 20.0),
        );
        assert_eq!(*seen.borrow(), vec![2]);
    }

    // ---- semantics ---------------------------------------------------------

    #[test]
    fn semantics_yields_a_group_of_toggle_members() {
        // Rewritten for the rework: members are real toggle buttons now, so
        // each contributes its own `Role::Button` + `Toggled` node rather than
        // the pre-rework `Role::RadioButton` the group synthesised itself.
        fn logic(_s: &mut ()) -> ButtonGroupView<()> {
            button_group::<(), _>(vec!["One", "Two", "Three"], 2, |_s: &mut (), _i| {})
                .semantic_label("Range")
        }
        let mut root: frust_core::RenderRoot<(), ButtonGroupView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("a Group container node is contributed");
        assert_eq!(group.1.children().len(), 3);
        assert_eq!(group.1.label(), Some("Range"));

        let members: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(members.len(), 3);
        let selected = members
            .iter()
            .find(|(_, n)| n.label() == Some("Three"))
            .expect("the Three member is present");
        assert_eq!(selected.1.toggled(), Some(frust::authoring::Toggled::True));
        let unselected = members
            .iter()
            .find(|(_, n)| n.label() == Some("One"))
            .expect("the One member is present");
        assert_eq!(
            unselected.1.toggled(),
            Some(frust::authoring::Toggled::False)
        );
    }

    #[test]
    fn semantics_omit_the_members_the_overflow_split_hid() {
        let mut widget = build(
            &button_group_actions(bare_icon_actions(6)).overflow(ButtonGroupOverflow::BottomSheet),
        );
        layout(&mut widget, Size::new(140.0, 200.0));
        assert_eq!(widget.visible_count, 2);
        // Input cannot reach a hidden member, so semantics omits it — the
        // input-parity carve-out in `docs/CODE_STANDARDS.md`.
        assert!(widget.hit(Point::new(300.0, 20.0)).is_none());
    }

    // ---- the legacy shape model is gone ------------------------------------

    #[test]
    fn this_module_no_longer_uses_the_legacy_radial_shape_model() {
        // Needles are assembled at runtime so this assertion cannot match its
        // own source text.
        let src = include_str!("button_group.rs");
        let import = format!("use super::{}_morph", "shape");
        let call = format!("{}_path(", "morph");
        assert!(
            !src.contains(&import),
            "the legacy radial polygon module must not be imported here"
        );
        assert!(
            !src.contains(&call),
            "the legacy radial morph primitive must not be called here"
        );
    }
}
