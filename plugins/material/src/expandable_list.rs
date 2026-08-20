// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/lists/ — components/m3e_expandable_item.dart,
//   components/m3e_expandable_item_body.dart,
//   components/m3e_expandable_list_base.dart,
//   styles/m3e_expandable_style.dart, styles/m3e_list_theme.dart's
//   `M3EListExpandableTheme`, enums/m3e_expandable_enums.dart (retrieved
//   2026-08-19). The reference's own item is in turn ported from
//   https://github.com/Mudit200408/m3e_expandable (MIT, © 2026 Mudit
//   Purohit) — already credited in `plugins/material/NOTICE`.
// The offstage double-measurement overlay, the tap-body/tap-icon interaction
// switches, tooltips, ink splash/factory overrides, and the per-progress
// header/body builders stay unported — see the module docs' Not ported.

//! The M3E **expandable list**: a stack of card-backed disclosure items, each a
//! pressable header row with a rotating chevron over a spring-revealed body.
//!
//! [`expandable_list`] is *controlled*: the caller owns the expanded index set
//! and receives [`ExpandableListView::on_toggle`] with the pressed item's
//! index; the widget never mutates its own `expanded` (the same never-self-
//! mutating contract every control in this catalog follows, see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! # Reveal timeline, and the layout-skip trap
//!
//! Each item's body child is **always laid out at its full natural height** and
//! stays laid out throughout; the reveal is a `0..1` fraction of that measured
//! height applied twice — the item's reported height is `header + fraction ·
//! body_height` (layout), and the body is painted under a clip of exactly that
//! revealed band (paint). The animation therefore never re-measures the child
//! mid-flight, and a collapsed item still contributes its subtree to layout and
//! semantics.
//!
//! The fraction is driven by a spring ([`REVEAL_SPRING`], the reference's
//! `expressiveSpatialDefault` — `MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`)
//! launched fresh on every toggle from wherever the fraction currently reads,
//! so an interrupted reveal reverses smoothly rather than jumping.
//!
//! **The clock lives on [`PaintCtx`], but the height is computed in `layout`.**
//! That split is the known trap for this widget class: on the mobile intra-frame
//! layout skip (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate`), layout does not
//! re-run merely because paint asked for another frame, so a paint-only
//! `request_frame` would leave the revealed height frozen while the chevron kept
//! turning. While any item's reveal is still animating, `paint` therefore calls
//! [`PaintCtx::request_layout`] (which implies `request_frame`) — the next
//! frame relayouts and picks up the freshly-advanced fraction, on every platform,
//! gated or not. `Theme.motion.reduce_motion` snaps straight to the target and
//! requests neither. This is the same mechanism `frust_glyph::accordion`
//! documents, and `paint_requests_layout_on_every_reveal_frame` is its
//! regression guard here.
//!
//! # Accordion (single-open) vs independent
//!
//! Upstream's `M3EExpandableStateMixin.handleToggle` owns both modes behind an
//! `allowMultipleExpanded` flag. Since the set is app-owned here, that rule
//! ships as a pure controller function instead — [`toggle_expanded`] with an
//! [`ExpandMode`] — mirroring how [`crate::selection`]'s index-set controller
//! functions are shaped:
//!
//! ```
//! use std::collections::BTreeSet;
//! use frust_material::{ExpandMode, toggle_expanded};
//!
//! let set = |items: &[usize]| items.iter().copied().collect::<BTreeSet<usize>>();
//! let open = set(&[0]);
//! // Accordion: opening 2 closes 0.
//! assert_eq!(toggle_expanded(&open, 2, ExpandMode::SingleOpen), set(&[2]));
//! // Independent: both stay open.
//! assert_eq!(toggle_expanded(&open, 2, ExpandMode::Independent), set(&[0, 2]));
//! // Either way, toggling an already-open index closes it.
//! assert!(toggle_expanded(&open, 0, ExpandMode::SingleOpen).is_empty());
//! ```
//!
//! # Corner matrix and interaction radii
//!
//! Item corners follow the same position matrix [`crate::card_list`] resolves
//! ([`crate::card_position`]/[`crate::card_radii`]) — outer on a stack's
//! outward edges, inner on every adjoining one. Upstream additionally swaps the
//! *inner* radius while an item is hovered or pressed
//! ([`EXPANDABLE_HOVER_RADIUS`] / [`EXPANDABLE_PRESSED_RADIUS`]), and rounds an
//! expanded item uniformly when `expandedRadius` is set
//! ([`ExpandableListView::expanded_radius`]); both are ported.
//!
//! # Interaction
//!
//! The **header row alone** toggles (upstream's default `tapHeaderToToggle:
//! true`, `tapBodyToExpand`/`tapBodyToCollapse: false`) — a pointer in the
//! revealed body routes to the body child instead, so a control inside a body
//! stays live. The header press follows the catalog's fire-on-up-inside
//! contract, and hover follows the three-part claim → latch → self-correct
//! rule.
//!
//! # Semantics
//!
//! One [`Role::List`] container node carrying [`Action::Click`], with every
//! header and body pod's own node forwarded beneath it.
//! [`frust::authoring::SemanticsCtx`] exposes no seam for contributing a node at
//! a *child's* bounds, so a synthetic per-item `expanded` node would publish the
//! whole stack's rect for every row — the same trade
//! [`crate::card_list`] documents. An app that needs per-item expanded state in
//! the accessibility tree stacks single-item [`expandable_list`]s of its own
//! instead, one per row.
//!
//! # Not ported
//!
//! * **The offstage double-measurement overlay** (`_buildMeasurementOverlay`) —
//!   upstream builds the body twice offstage (at progress `0` and `1`) to learn
//!   a collapsed and an expanded content height, because its body *builder*
//!   takes the progress and may return different content at each end. This port
//!   takes a single body [`frust::authoring::AnyView`], so there is exactly one
//!   natural height to measure and the overlay has nothing to resolve.
//! * **`tapBodyToExpand`/`tapBodyToCollapse`/`tapIconToToggle`** and the
//!   tooltips/`useInkWell`/`splashColor`/`splashFactory`/`enableFeedback`
//!   knobs — the same customization surface [`crate::card_list`] leaves
//!   unported. The header is the toggle target, full stop.
//! * **`iconPlacement`, `expandIcon`/`collapseIcon`, `headerAlignment`,
//!   `bodyAlignment`, `margin`, `titleSubtitleGap`, `elevation`, `border`** —
//!   the chevron is trailing and fixed ([`crate::icons::EXPAND_MORE_ROUNDED`]),
//!   the header is start-aligned, and the card is flat and borderless, matching
//!   every default in `M3EExpandableStyle`.
//! * **The 40ms `TweenAnimationBuilder` radius cross-fade** between two
//!   identical radii — a no-op in the reference (`begin` and `end` are the same
//!   value); the resolved radii are applied directly here, the same
//!   simplification [`crate::card_list`] documents for `M3ECardRadiusMotion`.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CornerRadii, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx,
    View, Widget, any,
};
use frust::{AnimationController, FrameTime, IconData, SpringDesc, Theme, Tween};
use kurbo::{Affine, BezPath, Point, Size, Vec2};
use peniko::{Brush, Color};

use crate::card_list::{card_position, card_radii};
use crate::interaction::{HapticSignal, InteractionState, MaterialHaptics};
use crate::press::presses;

/// Outward corner radius of the first/last (and only) item
/// (`M3EListExpandableTheme.defaultOuterRadius`, 24dp — the same value
/// [`crate::CARD_LIST_OUTER_RADIUS`] carries, which is what makes an expandable
/// stack read like a card list).
pub const EXPANDABLE_OUTER_RADIUS: f64 = 24.0;
/// Corner radius of every adjoining item edge
/// (`M3EListExpandableTheme.defaultInnerRadius`, 4dp).
pub const EXPANDABLE_INNER_RADIUS: f64 = 4.0;
/// Inner corner radius while an item is hovered
/// (`M3EListExpandableTheme.defaultHoverRadius`, 10dp).
pub const EXPANDABLE_HOVER_RADIUS: f64 = 10.0;
/// Inner corner radius while an item is pressed
/// (`M3EListExpandableTheme.defaultPressedRadius`, 4dp).
pub const EXPANDABLE_PRESSED_RADIUS: f64 = 4.0;
/// Vertical gap between adjacent items
/// (`M3EListExpandableTheme.defaultGap`, 4dp).
pub const EXPANDABLE_GAP: f64 = 4.0;
/// Total chevron rotation across a full reveal, in radians
/// (`M3EListExpandableTheme.defaultIconRotationAngle`, `math.pi`) — the
/// down-pointing `expand_more` glyph ends up pointing up.
pub const EXPANDABLE_ICON_ROTATION: f64 = std::f64::consts::PI;

/// Header horizontal padding, in logical px
/// (`M3EListExpandableTheme.defaultHeaderPadding`'s L/R).
const HEADER_PAD_X: f64 = 16.0;
/// Header top padding, in logical px (that same `EdgeInsets`' top).
const HEADER_PAD_TOP: f64 = 14.0;
/// Header bottom padding, in logical px (that same `EdgeInsets`' bottom).
const HEADER_PAD_BOTTOM: f64 = 2.0;
/// Body horizontal padding, in logical px
/// (`M3EListExpandableTheme.defaultBodyPadding`'s L/R).
const BODY_PAD_X: f64 = 16.0;
/// Body top padding, in logical px (that same `EdgeInsets`' top — zero, since
/// the header's own bottom padding already separates them).
const BODY_PAD_TOP: f64 = 0.0;
/// Body bottom padding, in logical px (that same `EdgeInsets`' bottom).
const BODY_PAD_BOTTOM: f64 = 20.0;
/// Chevron glyph size, in logical px (Flutter's default `Icon` size, which
/// `M3EExpandableStyle.expandIcon`'s bare `Icon(...)` inherits).
const CHEVRON_SIZE: f64 = 24.0;
/// Padding around the chevron on every side, in logical px
/// (`M3EListExpandableTheme.defaultIconPadding`, `EdgeInsets.all(8)`).
const CHEVRON_PAD: f64 = 8.0;
/// The trailing slot the chevron and its padding reserve in a header row.
const CHEVRON_SLOT: f64 = CHEVRON_SIZE + CHEVRON_PAD * 2.0;

/// Fallback item fill (a theme resolves this from
/// `colors.surface_container_highest`) — the same unthemed value
/// [`crate::card_list`] falls back to.
const FILLED_CONTAINER: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Fallback state-layer content color (a theme resolves this from
/// `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Fallback chevron color (a theme resolves this from
/// `colors.on_surface_variant`).
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);

/// The reveal spring: `MaterialSpringMotion.expressiveSpatialDefault`
/// (stiffness 380, ζ 0.8) — the reference's own
/// `M3EListExpandableTheme.expandMotion`/`collapseMotion` default, equal to
/// [`crate::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`]. Restated as a
/// `SpringDesc` here because a `fling` needs one and this crate's event/paint
/// code never reads a `Theme` for motion (the same shape
/// [`crate::split_button`]'s `CHEVRON_SPRING` takes; a test pins the equality).
pub const REVEAL_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.8,
};

/// Nominal period seeding a reveal [`AnimationController`]'s clock; the motion
/// is spring-driven via `fling`, so this backs construction only.
const REVEAL_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity for a reveal leg. Sub-visible on purpose: the spring's shape
/// comes from its displacement (`x0`), so only the sign matters — and it is
/// always positive here, because the leg runs `0 → 1` in *progress-toward-the-
/// new-target* space, never in reveal space (see [`Reveal`]). The same
/// sign-only convention [`crate::switch`]'s `RELEASE_VELOCITY` documents.
const REVEAL_LAUNCH_VELOCITY: f64 = 1e-3;

// ---- Controller: pure functions over an app-owned `BTreeSet<usize>` ------

/// How [`toggle_expanded`] treats the items that are already open
/// (`M3EExpandableListBase.allowMultipleExpanded`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExpandMode {
    /// Accordion: opening an item closes every other one
    /// (`allowMultipleExpanded: false`, the reference's default).
    #[default]
    SingleOpen,
    /// Independent: each item opens and closes on its own
    /// (`allowMultipleExpanded: true`).
    Independent,
}

/// Whether `index` is expanded (`M3EExpandableStateMixin.isExpanded`).
pub fn is_expanded(expanded: &BTreeSet<usize>, index: usize) -> bool {
    expanded.contains(&index)
}

/// The expanded set with `index`'s membership flipped under `mode`
/// (`M3EExpandableStateMixin.handleToggle`).
///
/// Collapsing is mode-independent: an already-expanded `index` is simply
/// removed. Expanding under [`ExpandMode::SingleOpen`] clears the set first, so
/// exactly one item is ever open — the accordion rule.
pub fn toggle_expanded(
    expanded: &BTreeSet<usize>,
    index: usize,
    mode: ExpandMode,
) -> BTreeSet<usize> {
    if expanded.contains(&index) {
        let mut next = expanded.clone();
        next.remove(&index);
        return next;
    }
    let mut next = match mode {
        ExpandMode::SingleOpen => BTreeSet::new(),
        ExpandMode::Independent => expanded.clone(),
    };
    next.insert(index);
    next
}

// ---- The reveal driver ----------------------------------------------------

/// One item's `0..1` reveal fraction, spring-driven.
///
/// The controller runs a plain `0 → 1` leg (its `fling` target is always `1.0`)
/// and [`Tween`] maps that leg onto `from → target` in reveal space, so a
/// retarget mid-flight starts a fresh leg from wherever the fraction currently
/// reads instead of jumping. The controller's value is read **clamped**: this
/// spring is under-damped (ζ 0.8) and an overshoot past `1.0` would reveal a
/// band taller than the measured body — empty space below the content — which
/// is why the reveal clamps where [`crate::switch`]'s spatial travel
/// deliberately does not.
struct Reveal {
    /// The authoritative fraction, always in `0.0..=1.0`.
    value: f64,
    /// The fraction the current leg is heading for (`0.0` or `1.0`).
    target: f64,
    /// Maps the controller's `0 → 1` leg onto `from → target`.
    tween: Tween<f64>,
    driver: AnimationController,
}

impl Reveal {
    /// A driver resting at `initial` (`0.0` collapsed, `1.0` expanded), with no
    /// leg in flight — an item built already-expanded shows its body on the
    /// first frame rather than animating into it, matching upstream's
    /// `..value = widget.isExpanded ? 1.0 : 0.0` seed.
    fn new(initial: f64) -> Self {
        Self {
            value: initial,
            target: initial,
            tween: Tween::new(initial, initial),
            driver: AnimationController::new(REVEAL_PERIOD),
        }
    }

    /// Launch a fresh leg toward `target` (a no-op if already heading there).
    fn set_target(&mut self, target: f64) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.tween = Tween::new(self.value, target);
        self.driver = AnimationController::new(REVEAL_PERIOD);
        self.driver.fling(REVEAL_LAUNCH_VELOCITY, REVEAL_SPRING);
    }

    /// Jump straight to the target, cancelling any leg (the reduce-motion path).
    fn snap(&mut self) {
        self.value = self.target;
        self.tween = Tween::new(self.target, self.target);
        self.driver.stop();
    }

    /// Advance one frame, returning whether the leg is still in flight.
    fn advance(&mut self, now: FrameTime) -> bool {
        let animating = self.driver.advance(now);
        self.value = self.tween.lerp(self.driver.value_clamped()).clamp(0.0, 1.0);
        animating
    }
}

// ---- The view -------------------------------------------------------------

/// One expandable item: a header row over a body revealed when it expands.
pub struct ExpandableItem<State: 'static> {
    header: AnyView<State>,
    body: AnyView<State>,
}

/// Build an [`ExpandableItem`] from a `header` row and the `body` it reveals
/// (upstream's `headerBuilder`/`bodyBuilder` pair, minus their progress
/// argument — see the [module docs](self)' Not ported).
pub fn expandable_item<State: 'static, H: View<State>, B: View<State>>(
    header: H,
    body: B,
) -> ExpandableItem<State> {
    ExpandableItem {
        header: any(header),
        body: any(body),
    }
}

/// A view-held, typed per-index toggle callback (erased on build).
type OnToggleIndex<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative M3E expandable list. See the [module docs](self).
pub struct ExpandableListView<State: 'static> {
    items: Vec<ExpandableItem<State>>,
    expanded: BTreeSet<usize>,
    outer_radius: f64,
    inner_radius: f64,
    hover_radius: f64,
    pressed_radius: f64,
    expanded_radius: Option<f64>,
    gap: f64,
    color: Option<Color>,
    haptic: HapticSignal,
    on_toggle: Option<OnToggleIndex<State>>,
}

/// Stack `items` as an expandable list. Controlled: pass the app-owned
/// [`expanded`](ExpandableListView::expanded) set and handle
/// [`on_toggle`](ExpandableListView::on_toggle), resolving the new set through
/// [`toggle_expanded`].
pub fn expandable_list<State: 'static>(
    items: impl IntoIterator<Item = ExpandableItem<State>>,
) -> ExpandableListView<State> {
    ExpandableListView {
        items: items.into_iter().collect(),
        expanded: BTreeSet::new(),
        outer_radius: EXPANDABLE_OUTER_RADIUS,
        inner_radius: EXPANDABLE_INNER_RADIUS,
        hover_radius: EXPANDABLE_HOVER_RADIUS,
        pressed_radius: EXPANDABLE_PRESSED_RADIUS,
        expanded_radius: None,
        gap: EXPANDABLE_GAP,
        color: None,
        haptic: HapticSignal::None,
        on_toggle: None,
    }
}

impl<State: 'static> ExpandableListView<State> {
    /// Mark these indices expanded (`initiallyExpanded`, then every later
    /// app-resolved set). The widget never writes this back.
    pub fn expanded(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.expanded = indices.into_iter().collect();
        self
    }

    /// Override the outward corner radius of the first/last (and only) item.
    /// Defaults to [`EXPANDABLE_OUTER_RADIUS`].
    pub fn outer_radius(mut self, radius: f64) -> Self {
        self.outer_radius = radius;
        self
    }

    /// Override the radius of every adjoining item edge. Defaults to
    /// [`EXPANDABLE_INNER_RADIUS`].
    pub fn inner_radius(mut self, radius: f64) -> Self {
        self.inner_radius = radius;
        self
    }

    /// Override the inner radius used while an item is hovered. Defaults to
    /// [`EXPANDABLE_HOVER_RADIUS`].
    pub fn hover_radius(mut self, radius: f64) -> Self {
        self.hover_radius = radius;
        self
    }

    /// Override the inner radius used while an item is pressed. Defaults to
    /// [`EXPANDABLE_PRESSED_RADIUS`].
    pub fn pressed_radius(mut self, radius: f64) -> Self {
        self.pressed_radius = radius;
        self
    }

    /// Round an *expanded* item uniformly at `radius`, overriding its position
    /// radii entirely (`M3EExpandableStyle.expandedRadius`). Unset by default,
    /// as upstream.
    pub fn expanded_radius(mut self, radius: f64) -> Self {
        self.expanded_radius = Some(radius);
        self
    }

    /// Override the vertical gap between adjacent items. Defaults to
    /// [`EXPANDABLE_GAP`].
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap;
        self
    }

    /// Replace the item fill (`M3EExpandableStyle.color`), which otherwise
    /// resolves from `colors.surface_container_highest`.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Set the haptic fired immediately before
    /// [`on_toggle`](Self::on_toggle) (`M3EExpandableStyle.haptic`). Defaults
    /// to [`HapticSignal::None`].
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Fire `on_toggle` with an item's index when its header is released
    /// inside itself. Without this the stack is inert chrome: no press state,
    /// no hover claim, and pointer events route straight into the children.
    pub fn on_toggle<F: Fn(&mut State, usize) + 'static>(mut self, on_toggle: F) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    /// The header/body views flattened into one paint-ordered child list —
    /// `[header_0, body_0, header_1, body_1, ...]`, the order
    /// [`ExpandableListWidget::pods`] retains.
    fn flat_views(&self) -> Vec<&AnyView<State>> {
        self.items
            .iter()
            .flat_map(|item| [&item.header, &item.body])
            .collect()
    }
}

// ---- The widget -----------------------------------------------------------

/// One item's measured geometry in the stack's local space, recorded during
/// layout and read by both paint and the event pass's hit test.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ItemGeometry {
    /// Local y of the item's top edge.
    y: f64,
    /// The header row's height (the toggle target's extent).
    header: f64,
    /// The body's full natural height, padding included — the reveal's `1.0`.
    body_full: f64,
    /// The item's currently reported height (`header + reveal · body_full`).
    height: f64,
}

/// The retained widget for an [`ExpandableListView`].
pub struct ExpandableListWidget {
    /// `[header_0, body_0, header_1, body_1, ...]` — see
    /// [`ExpandableListView::flat_views`].
    pods: Vec<ChildPod>,
    expanded: BTreeSet<usize>,
    reveals: Vec<Reveal>,
    geometry: Vec<ItemGeometry>,
    outer_radius: f64,
    inner_radius: f64,
    hover_radius: f64,
    pressed_radius: f64,
    expanded_radius: Option<f64>,
    gap: f64,
    color: Option<Color>,
    haptic: HapticSignal,
    interactive: bool,
    /// The laid-out stack width; item rects span it fully.
    width: f64,
    /// The item a `Down` armed (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`/loss of interactivity.
    armed: Option<usize>,
    /// The item currently painting a state-layer overlay.
    active: Option<usize>,
    state: InteractionState,
    /// The chevron glyph's `(path, design size)`, resolved once at build.
    chevron: Option<(BezPath, f64)>,
    on_toggle: Option<ErasedArgCallback<usize>>,
}

/// Return `color` with its alpha channel replaced by `alpha` (the same helper
/// [`crate::card_list`] carries).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved item fill. Themed: `colors.surface_container_highest`, unless
/// an explicit `override_color` replaces it.
fn resolve_container(theme: Option<&Theme>, override_color: Option<Color>) -> Color {
    if let Some(color) = override_color {
        return color;
    }
    match theme {
        Some(theme) => theme.scheme().surface_container_highest,
        None => FILLED_CONTAINER,
    }
}

/// The resolved state-layer content color. Themed: `colors.on_surface`.
fn resolve_content(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

/// The resolved chevron color. Themed: `colors.on_surface_variant`.
fn resolve_chevron(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface_variant,
        None => ON_SURFACE_VARIANT,
    }
}

impl<State: 'static> View<State> for ExpandableListView<State> {
    type Element = ExpandableListWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ExpandableListWidget {
        let pods = self
            .flat_views()
            .into_iter()
            .map(|view| frust::authoring::build_child(view, ctx))
            .collect();
        let reveals = (0..self.items.len())
            .map(|index| {
                Reveal::new(if self.expanded.contains(&index) {
                    1.0
                } else {
                    0.0
                })
            })
            .collect();
        ExpandableListWidget {
            pods,
            expanded: self.expanded.clone(),
            reveals,
            geometry: vec![ItemGeometry::default(); self.items.len()],
            outer_radius: self.outer_radius,
            inner_radius: self.inner_radius,
            hover_radius: self.hover_radius,
            pressed_radius: self.pressed_radius,
            expanded_radius: self.expanded_radius,
            gap: self.gap,
            color: self.color,
            haptic: self.haptic,
            interactive: self.on_toggle.is_some(),
            width: 0.0,
            armed: None,
            active: None,
            state: InteractionState::new(),
            chevron: Some(IconData::from(crate::icons::EXPAND_MORE_ROUNDED).resolve()),
            on_toggle: self
                .on_toggle
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ExpandableListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let prev_flat = prev.flat_views();
        let next_flat = self.flat_views();
        let mut flags = frust::authoring::rebuild_children(
            &prev_flat,
            &next_flat,
            &mut element.pods,
            ctx,
            |view| *view,
            |_| None,
        );

        if prev.items.len() != self.items.len() {
            // A structural change invalidates the recorded press (the armed
            // index may now name a different item) and the per-item reveal
            // drivers, which are positional.
            element.armed = None;
            element.active = None;
            element.state.set_pressed(false);
            element.state.set_hovered(false);
            element.reveals = (0..self.items.len())
                .map(|index| {
                    Reveal::new(if self.expanded.contains(&index) {
                        1.0
                    } else {
                        0.0
                    })
                })
                .collect();
            element.geometry = vec![ItemGeometry::default(); self.items.len()];
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if element.expanded != self.expanded {
            for (index, reveal) in element.reveals.iter_mut().enumerate() {
                reveal.set_target(if self.expanded.contains(&index) {
                    1.0
                } else {
                    0.0
                });
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.expanded = self.expanded.clone();

        if element.outer_radius != self.outer_radius
            || element.inner_radius != self.inner_radius
            || element.hover_radius != self.hover_radius
            || element.pressed_radius != self.pressed_radius
            || element.expanded_radius != self.expanded_radius
            || element.color != self.color
        {
            element.outer_radius = self.outer_radius;
            element.inner_radius = self.inner_radius;
            element.hover_radius = self.hover_radius;
            element.pressed_radius = self.pressed_radius;
            element.expanded_radius = self.expanded_radius;
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if element.gap != self.gap {
            element.gap = self.gap;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let now_interactive = self.on_toggle.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            if !now_interactive {
                // Losing the interactive surface mid-gesture must not leave a
                // dangling press or hover behind.
                element.armed = None;
                element.active = None;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        element.haptic = self.haptic;
        element.on_toggle = self
            .on_toggle
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut ExpandableListWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.flat_views().into_iter().zip(element.pods.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl ExpandableListWidget {
    /// How many items the stack holds (two pods each).
    fn item_count(&self) -> usize {
        self.pods.len() / 2
    }

    /// The item whose *header row* contains local `pos`, if any — a position in
    /// a revealed body, or in the gap between two items, belongs to none.
    fn header_at(&self, pos: Point) -> Option<usize> {
        if pos.x < 0.0 || pos.x >= self.width {
            return None;
        }
        self.geometry
            .iter()
            .position(|g| pos.y >= g.y && pos.y < g.y + g.header)
    }

    /// The radii item `index` takes: its stack position's matrix, with the
    /// inner radius swapped for the hover/pressed one, or a uniform
    /// [`ExpandableListView::expanded_radius`] while expanded.
    fn radii_at(&self, index: usize) -> CornerRadii {
        if let Some(radius) = self.expanded_radius
            && self.expanded.contains(&index)
        {
            return CornerRadii::uniform(radius);
        }
        let inner = if self.armed == Some(index) && self.state.pressed {
            self.pressed_radius
        } else if self.active == Some(index) && self.state.hovered {
            self.hover_radius
        } else {
            self.inner_radius
        };
        card_radii(
            card_position(index, self.item_count()),
            self.outer_radius,
            inner,
        )
    }

    /// Clear whatever gesture state a press left behind.
    fn clear_press(&mut self) {
        self.armed = None;
        self.state.set_pressed(false);
        if !self.state.is_active() {
            self.active = None;
        }
    }

    /// The chevron path for item `index`, positioned and rotated for the
    /// current reveal fraction, in the widget's local space.
    fn chevron_path(&self, index: usize) -> Option<BezPath> {
        let (path, design) = self.chevron.as_ref()?;
        if *design <= 0.0 {
            return None;
        }
        let geometry = self.geometry.get(index)?;
        let center = Point::new(
            self.width - HEADER_PAD_X - CHEVRON_SLOT / 2.0,
            geometry.y + HEADER_PAD_TOP + CHEVRON_SLOT / 2.0,
        );
        let angle = self.reveals.get(index).map_or(0.0, |r| r.value) * EXPANDABLE_ICON_ROTATION;
        let scale = CHEVRON_SIZE / design;
        // Rotate about the glyph's own center: recentre the design box on the
        // origin, scale, rotate, then translate onto the slot's center.
        let transform = Affine::translate(Vec2::new(center.x, center.y))
            * Affine::rotate(angle)
            * Affine::scale(scale)
            * Affine::translate(Vec2::new(-design / 2.0, -design / 2.0));
        Some(transform * path.clone())
    }
}

impl Widget for ExpandableListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        self.width = width;
        let header_width = (width - HEADER_PAD_X * 2.0 - CHEVRON_SLOT).max(0.0);
        let body_width = (width - BODY_PAD_X * 2.0).max(0.0);

        let count = self.item_count();
        self.geometry.resize(count, ItemGeometry::default());
        let mut y = 0.0;
        for index in 0..count {
            let header_bc = BoxConstraints::new(
                Size::new(header_width, 0.0),
                Size::new(header_width, f64::INFINITY),
            );
            let header_size = self.pods[index * 2].layout_child(ctx, &header_bc);
            // The chevron slot sets the row's floor: a one-line header must
            // still leave the glyph its own 40dp box.
            let header = header_size.height.max(CHEVRON_SLOT) + HEADER_PAD_TOP + HEADER_PAD_BOTTOM;
            self.pods[index * 2].set_origin(Point::new(HEADER_PAD_X, y + HEADER_PAD_TOP));

            // The body is always laid out at its full natural height; the
            // reveal is a fraction of that, never a re-measure.
            let body_bc = BoxConstraints::new(
                Size::new(body_width, 0.0),
                Size::new(body_width, f64::INFINITY),
            );
            let body_size = self.pods[index * 2 + 1].layout_child(ctx, &body_bc);
            let body_full = body_size.height + BODY_PAD_TOP + BODY_PAD_BOTTOM;
            self.pods[index * 2 + 1].set_origin(Point::new(BODY_PAD_X, y + header + BODY_PAD_TOP));

            let reveal = self.reveals.get(index).map_or(0.0, |r| r.value);
            let height = header + (reveal * body_full).max(0.0);
            self.geometry[index] = ItemGeometry {
                y,
                header,
                body_full,
                height,
            };
            y += height;
            if index + 1 < count {
                y += self.gap;
            }
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every theme read happens before `ctx` is taken mutably below
        // (`paint_child`) — the ordering `card_list`/`card` share.
        let theme = Theme::from_paint_ctx(ctx);
        let container = resolve_container(theme, self.color);
        let content = resolve_content(theme);
        let chevron_color = resolve_chevron(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);

        // Advance every reveal. The item height is computed in `layout` from
        // these fractions, so an in-flight reveal needs an explicit relayout
        // request, not merely another frame (see the module docs).
        let now = ctx.frame_time();
        let mut animating = false;
        for reveal in &mut self.reveals {
            if reduce_motion {
                reveal.snap();
            } else if reveal.advance(now) {
                animating = true;
            }
        }
        if animating {
            ctx.request_layout();
        }

        // Authoritative hover read, self-correcting the latched flag — inert
        // while non-interactive (`docs/CODE_STANDARDS.md`'s three-part hover
        // contract).
        let hovered = self.interactive && ctx.is_hovered();
        self.state.set_hovered(hovered);
        if !hovered && !self.state.pressed {
            self.active = None;
        }

        let origin = ctx.origin();
        let overlay_opacity = if self.interactive {
            self.state.resolve_opacity()
        } else {
            0.0
        };
        let count = self.item_count();
        let radii: Vec<CornerRadii> = (0..count).map(|index| self.radii_at(index)).collect();
        let chevrons: Vec<Option<BezPath>> =
            (0..count).map(|index| self.chevron_path(index)).collect();

        for index in 0..count {
            let Some(geometry) = self.geometry.get(index).copied() else {
                continue;
            };
            let absolute = Point::new(origin.x, origin.y + geometry.y);
            let size = Size::new(self.width, geometry.height);
            scene.fill_rounded_rect_radii(absolute, size, radii[index], container);
            if overlay_opacity > 0.0 && self.active == Some(index) {
                scene.fill_rounded_rect_radii(
                    absolute,
                    size,
                    radii[index],
                    with_alpha(content, overlay_opacity),
                );
            }

            self.pods[index * 2].paint_child(ctx, scene);

            let revealed = geometry.height - geometry.header;
            if revealed > 0.0 {
                // Only the revealed band of the body shows; the child stays
                // laid out at its full height behind this clip.
                scene.push_clip(
                    Point::new(absolute.x, absolute.y + geometry.header),
                    Size::new(self.width, revealed),
                );
                self.pods[index * 2 + 1].paint_child(ctx, scene);
                scene.pop_clip();
            }

            if let Some(path) = &chevrons[index] {
                scene.fill_path(origin, path, &Brush::Solid(chevron_color));
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A broadcast is not user input: it reaches every child ahead of the
        // capture/hit-test branches and is never consumed.
        if event.is_broadcast() || !self.interactive {
            return frust::authoring::route_event(&mut self.pods, ctx, event);
        }
        // Key/Ime are focus-routed; the stack itself has no key handling.
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event(&mut self.pods, ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                let Some(index) = self.header_at(p.position).filter(|_| presses(p)) else {
                    // Outside every header row (a revealed body, or the gap
                    // between two items): the children own it.
                    return frust::authoring::route_event(&mut self.pods, ctx, event);
                };
                self.armed = Some(index);
                self.active = Some(index);
                self.state.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(armed) = self.armed else {
                    // No capture: route first, then claim hover as a fallback —
                    // an ancestor claiming ahead of its children would make
                    // every descendant ineligible for the pass.
                    let result = frust::authoring::route_event(&mut self.pods, ctx, event);
                    let over = self.header_at(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                    }
                    let changed = self.state.set_hovered(over.is_some()) || self.active != over;
                    self.active = over;
                    if changed {
                        ctx.request_redraw();
                    }
                    return result;
                };
                let still_on_header = self.header_at(p.position) == Some(armed);
                if self.state.set_pressed(still_on_header) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed else {
                    return frust::authoring::route_event(&mut self.pods, ctx, event);
                };
                if self.header_at(p.position) == Some(armed) {
                    if self.haptic != HapticSignal::None {
                        MaterialHaptics::fire(self.haptic);
                    }
                    if let Some(on_toggle) = self.on_toggle.as_mut() {
                        (on_toggle)(ctx, armed);
                    }
                }
                self.clear_press();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return frust::authoring::route_event(&mut self.pods, ctx, event);
                }
                // A `Cancel` arm only clears internal flags — never state.
                self.clear_press();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::List,
            |node| {
                if self.interactive {
                    node.add_action(Action::Click);
                }
            },
            |ctx| {
                for pod in &self.pods {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust_widgets::test_support::leaf_any;
    use kurbo::Shape;
    use std::any::Any;

    const WIDTH: f64 = 320.0;
    const HEADER_CHILD: f64 = 20.0;
    const BODY_CHILD: f64 = 60.0;

    /// The header row height a [`HEADER_CHILD`]-tall header child yields — the
    /// chevron slot is the floor, so the child's own 20dp never wins.
    fn header_height() -> f64 {
        HEADER_CHILD.max(CHEVRON_SLOT) + HEADER_PAD_TOP + HEADER_PAD_BOTTOM
    }

    fn body_height() -> f64 {
        BODY_CHILD + BODY_PAD_TOP + BODY_PAD_BOTTOM
    }

    fn items(count: usize) -> Vec<ExpandableItem<()>> {
        (0..count)
            .map(|_| ExpandableItem {
                header: leaf_any(40.0, HEADER_CHILD),
                body: leaf_any(40.0, BODY_CHILD),
            })
            .collect()
    }

    fn build<S: 'static>(view: &ExpandableListView<S>) -> ExpandableListWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ExpandableListWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut lctx,
            &BoxConstraints::new(Size::new(WIDTH, 0.0), Size::new(WIDTH, f64::INFINITY)),
        )
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, CornerRadii, Color)>,
        clips: Vec<(Point, Size)>,
        paths: Vec<BezPath>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.rrects.push((o, s, radii, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn fill_path(&mut self, origin: Point, path: &BezPath, _brush: &Brush) {
            self.paths
                .push(Affine::translate(Vec2::new(origin.x, origin.y)) * path.clone());
        }
    }

    /// Paint at `t_ms` with a real clock, returning `(recorder, needs_layout)`.
    fn paint_at(
        w: &mut ExpandableListWidget,
        t_ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let size = Size::new(WIDTH, 4000.0);
        let mut ctx =
            PaintCtx::for_test(Point::ZERO, size, FrameTime::from_nanos(t_ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        w.paint(&mut ctx, &mut rec);
        let needs_layout = ctx.needs_layout();
        (rec, needs_layout)
    }

    fn dispatch(
        w: &mut ExpandableListWidget,
        state: &mut dyn Any,
        phase: PointerPhase,
        pos: Point,
    ) -> EventResult {
        let event = InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: pos,
            button: frust::authoring::PointerButton::Primary,
        });
        let mut ctx = EventCtx::new(state, Point::ZERO, Size::new(WIDTH, 4000.0));
        w.event(&mut ctx, &event)
    }

    // ---- The accordion controller ----------------------------------------

    #[test]
    fn single_open_closes_every_other_item() {
        let open: BTreeSet<usize> = [0, 1].into_iter().collect();
        let next = toggle_expanded(&open, 2, ExpandMode::SingleOpen);
        assert_eq!(next, [2].into_iter().collect::<BTreeSet<usize>>());
    }

    #[test]
    fn independent_mode_keeps_the_others_open() {
        let open: BTreeSet<usize> = [0, 1].into_iter().collect();
        let next = toggle_expanded(&open, 2, ExpandMode::Independent);
        assert_eq!(next, [0, 1, 2].into_iter().collect::<BTreeSet<usize>>());
    }

    #[test]
    fn collapsing_is_mode_independent() {
        let open: BTreeSet<usize> = [0, 1].into_iter().collect();
        assert_eq!(
            toggle_expanded(&open, 1, ExpandMode::SingleOpen),
            [0].into_iter().collect::<BTreeSet<usize>>()
        );
        assert_eq!(
            toggle_expanded(&open, 1, ExpandMode::Independent),
            [0].into_iter().collect::<BTreeSet<usize>>()
        );
    }

    #[test]
    fn is_expanded_reads_the_app_owned_set() {
        let open: BTreeSet<usize> = [3].into_iter().collect();
        assert!(is_expanded(&open, 3));
        assert!(!is_expanded(&open, 0));
    }

    #[test]
    fn single_open_is_the_default_mode() {
        assert_eq!(ExpandMode::default(), ExpandMode::SingleOpen);
    }

    // ---- Layout: the reveal is a fraction of a fully-measured body --------

    #[test]
    fn a_collapsed_item_reports_only_its_header_height() {
        let view: ExpandableListView<()> = expandable_list(items(1));
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size.height, header_height());
        assert_eq!(w.geometry[0].header, header_height());
        assert_eq!(
            w.geometry[0].body_full,
            body_height(),
            "the body is measured in full even while collapsed"
        );
    }

    #[test]
    fn an_item_built_expanded_reports_header_plus_full_body_on_the_first_layout() {
        let view: ExpandableListView<()> = expandable_list(items(1)).expanded([0]);
        let mut w = build(&view);
        let size = layout(&mut w);
        assert!((size.height - (header_height() + body_height())).abs() < 1e-9);
    }

    #[test]
    fn a_mid_reveal_height_sits_between_collapsed_and_expanded() {
        let view: ExpandableListView<()> = expandable_list(items(1)).expanded([0]);
        let mut w = build(&view);
        layout(&mut w);
        w.reveals[0].value = 0.5;
        let size = layout(&mut w);
        assert!((size.height - (header_height() + 0.5 * body_height())).abs() < 1e-9);
    }

    #[test]
    fn items_stack_with_the_gap_between_them() {
        let view: ExpandableListView<()> = expandable_list(items(3));
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size.height, header_height() * 3.0 + EXPANDABLE_GAP * 2.0);
        assert_eq!(w.geometry[0].y, 0.0);
        assert_eq!(w.geometry[1].y, header_height() + EXPANDABLE_GAP);
    }

    // ---- The reveal timeline, and the layout-skip guard -------------------

    #[test]
    fn paint_requests_layout_on_every_reveal_frame_and_stops_once_settled() {
        // The regression guard for the layout-skip trap: the revealed height is
        // computed in `layout` from a fraction advanced in `paint`, so every
        // in-flight frame must ask for a relayout — a paint-only frame request
        // would freeze the height under the mobile intra-frame layout skip.
        let collapsed: ExpandableListView<()> = expandable_list(items(1));
        let mut w = build(&collapsed);
        layout(&mut w);
        let (_, settled) = paint_at(&mut w, 0, None);
        assert!(!settled, "an already-settled stack requests no layout");

        let expanded: ExpandableListView<()> = expandable_list(items(1)).expanded([0]);
        let mut counter = 0u64;
        <ExpandableListView<()> as View<()>>::rebuild(
            &expanded,
            &collapsed,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );

        let mut frames = 0u32;
        let mut heights = Vec::new();
        let mut t_ms = 0u64;
        loop {
            let (_, needs_layout) = paint_at(&mut w, t_ms, None);
            if !needs_layout {
                break;
            }
            frames += 1;
            // A real shell relayouts on seeing `needs_layout`.
            heights.push(layout(&mut w).height);
            t_ms += 16;
            assert!(
                frames < 600,
                "the reveal should settle well inside 600 frames"
            );
        }
        // The frame that settles the spring stops asking for layout, so record
        // its resting height too.
        heights.push(layout(&mut w).height);
        assert!(frames > 1, "the reveal spans more than one frame");
        let collapsed_h = header_height();
        let expanded_h = header_height() + body_height();
        // Clamp discipline: an under-damped spring (ζ 0.8) overshoots past its
        // target, and the reveal reads clamped — so no frame ever reports a
        // band taller than the measured body (empty space below the content).
        assert!(
            heights
                .iter()
                .all(|h| *h >= collapsed_h - 1e-9 && *h <= expanded_h + 1e-9),
            "every frame stays inside [{collapsed_h}, {expanded_h}]: {heights:?}"
        );
        // Monotonic rise up to the first frame that reaches the target (past
        // that, the clipped overshoot may dip back by a fraction of a px).
        let peak = heights
            .iter()
            .position(|h| *h >= expanded_h - 1e-9)
            .expect("the reveal reaches its target");
        assert!(peak > 0, "the first frame is not already fully revealed");
        assert!(
            heights[..=peak].windows(2).all(|pair| pair[1] > pair[0]),
            "the reveal rises monotonically to the target: {heights:?}"
        );
        let last = heights.last().copied().expect("at least one frame");
        assert!(
            (last - expanded_h).abs() < 1e-9,
            "the settled reveal lands exactly on the full body height"
        );
        // A settled stack stops asking.
        let (_, still) = paint_at(&mut w, t_ms + 16, None);
        assert!(!still, "settled: no more layout requests");
    }

    #[test]
    fn reduce_motion_snaps_the_reveal_and_requests_nothing() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let collapsed: ExpandableListView<()> = expandable_list(items(1));
        let mut w = build(&collapsed);
        layout(&mut w);
        let expanded: ExpandableListView<()> = expandable_list(items(1)).expanded([0]);
        let mut counter = 0u64;
        <ExpandableListView<()> as View<()>>::rebuild(
            &expanded,
            &collapsed,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        let (_, needs_layout) = paint_at(&mut w, 0, Some(&theme));
        assert_eq!(w.reveals[0].value, 1.0, "snapped straight to the target");
        assert!(!needs_layout, "a snapped reveal asks for no further layout");
    }

    #[test]
    fn only_the_toggled_item_animates() {
        let collapsed: ExpandableListView<()> = expandable_list(items(3));
        let mut w = build(&collapsed);
        layout(&mut w);
        let expanded: ExpandableListView<()> = expandable_list(items(3)).expanded([1]);
        let mut counter = 0u64;
        <ExpandableListView<()> as View<()>>::rebuild(
            &expanded,
            &collapsed,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        let mut t_ms = 0u64;
        loop {
            let (rec, needs_layout) = paint_at(&mut w, t_ms, None);
            assert!(
                rec.clips.len() <= 1,
                "only the toggled item ever reveals a body clip, got {}",
                rec.clips.len()
            );
            if !needs_layout {
                break;
            }
            layout(&mut w);
            t_ms += 16;
            assert!(t_ms < 10_000, "the reveal should settle well inside 10s");
        }
        assert_eq!(w.reveals[0].value, 0.0);
        assert_eq!(w.reveals[2].value, 0.0);
        assert_eq!(w.reveals[1].value, 1.0);
    }

    #[test]
    fn the_body_clip_covers_exactly_the_revealed_band() {
        let view: ExpandableListView<()> = expandable_list(items(1)).expanded([0]);
        let mut w = build(&view);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, 0, None);
        assert_eq!(rec.clips.len(), 1);
        let (clip_origin, clip_size) = rec.clips[0];
        assert_eq!(clip_origin.y, header_height());
        assert!((clip_size.height - body_height()).abs() < 1e-9);
        assert_eq!(clip_size.width, WIDTH);
    }

    #[test]
    fn a_collapsed_item_paints_no_body_clip() {
        let view: ExpandableListView<()> = expandable_list(items(2));
        let mut w = build(&view);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, 0, None);
        assert!(rec.clips.is_empty());
    }

    #[test]
    fn the_chevron_rotates_with_the_reveal() {
        let closed: ExpandableListView<()> = expandable_list(items(1));
        let mut w = build(&closed);
        layout(&mut w);
        let (rec_closed, _) = paint_at(&mut w, 0, None);
        let closed_box = rec_closed.paths[0].bounding_box();

        let open: ExpandableListView<()> = expandable_list(items(1)).expanded([0]);
        let mut w2 = build(&open);
        layout(&mut w2);
        let (rec_open, _) = paint_at(&mut w2, 0, None);
        let open_box = rec_open.paths[0].bounding_box();

        // A π rotation about the *slot* center point-reflects the glyph's ink
        // bounds through that center (the `expand_more` ink box is not itself
        // centered in its 24dp design box, so the two bboxes are not equal).
        let slot_x = WIDTH - HEADER_PAD_X - CHEVRON_SLOT / 2.0;
        let slot_y = HEADER_PAD_TOP + CHEVRON_SLOT / 2.0;
        assert!(
            (closed_box.center().x + open_box.center().x - 2.0 * slot_x).abs() < 1e-6,
            "closed {closed_box:?} / open {open_box:?} about x={slot_x}"
        );
        assert!(
            (closed_box.center().y + open_box.center().y - 2.0 * slot_y).abs() < 1e-6,
            "closed {closed_box:?} / open {open_box:?} about y={slot_y}"
        );
        assert_ne!(
            rec_closed.paths[0].to_svg(),
            rec_open.paths[0].to_svg(),
            "the chevron path must actually rotate"
        );
    }

    #[test]
    fn the_reveal_spring_matches_the_reference_preset() {
        let reference = crate::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT;
        assert_eq!(REVEAL_SPRING.mass, 1.0);
        assert_eq!(REVEAL_SPRING.stiffness, reference.stiffness);
        assert_eq!(REVEAL_SPRING.damping_ratio, reference.damping_ratio);
    }

    // ---- The corner matrix ------------------------------------------------

    #[test]
    fn item_corners_follow_the_card_list_position_matrix() {
        let view: ExpandableListView<()> = expandable_list(items(3));
        let mut w = build(&view);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, 0, None);
        let radii: Vec<CornerRadii> = rec.rrects.iter().map(|(_, _, r, _)| *r).collect();
        let (outer, inner) = (EXPANDABLE_OUTER_RADIUS, EXPANDABLE_INNER_RADIUS);
        assert_eq!(
            radii,
            vec![
                card_radii(crate::CardPosition::First, outer, inner),
                card_radii(crate::CardPosition::Middle, outer, inner),
                card_radii(crate::CardPosition::Last, outer, inner),
            ]
        );
    }

    #[test]
    fn an_expanded_radius_override_rounds_the_open_item_uniformly() {
        let view: ExpandableListView<()> = expandable_list(items(2))
            .expanded([0])
            .expanded_radius(28.0);
        let mut w = build(&view);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, 0, None);
        assert_eq!(rec.rrects[0].2, CornerRadii::uniform(28.0));
        assert_ne!(rec.rrects[1].2, CornerRadii::uniform(28.0));
    }

    #[test]
    fn a_pressed_item_swaps_in_the_pressed_inner_radius() {
        let view: ExpandableListView<()> = expandable_list(items(2)).on_toggle(|_: &mut (), _| {});
        let mut w = build(&view);
        layout(&mut w);
        let mut state = ();
        dispatch(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(10.0, header_height() / 2.0),
        );
        let expected = card_radii(
            crate::CardPosition::First,
            EXPANDABLE_OUTER_RADIUS,
            EXPANDABLE_PRESSED_RADIUS,
        );
        assert_eq!(w.radii_at(0), expected);
    }

    #[test]
    fn a_hovered_item_swaps_in_the_hover_inner_radius() {
        // The latch half of the hover contract: an uncaptured `Move` over a
        // header claims and latches, and the radii read it straight away.
        let view: ExpandableListView<()> = expandable_list(items(2)).on_toggle(|_: &mut (), _| {});
        let mut w = build(&view);
        layout(&mut w);
        let mut state = ();
        let second_y = w.geometry[1].y + 1.0;
        dispatch(
            &mut w,
            &mut state,
            PointerPhase::Move,
            Point::new(10.0, second_y),
        );
        assert_eq!(w.active, Some(1));
        assert!(w.state.hovered);
        assert_eq!(
            w.radii_at(1),
            card_radii(
                crate::CardPosition::Last,
                EXPANDABLE_OUTER_RADIUS,
                EXPANDABLE_HOVER_RADIUS,
            )
        );
        // Its unhovered sibling keeps the plain inner radius.
        assert_eq!(
            w.radii_at(0),
            card_radii(
                crate::CardPosition::First,
                EXPANDABLE_OUTER_RADIUS,
                EXPANDABLE_INNER_RADIUS,
            )
        );
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn a_header_press_released_inside_reports_its_index() {
        #[derive(Default)]
        struct S {
            toggled: Vec<usize>,
        }
        let view: ExpandableListView<S> = expandable_list((0..3).map(|_| {
            expandable_item(
                frust::SizedBox::<S>(Some(40.0), Some(HEADER_CHILD)),
                frust::SizedBox::<S>(Some(40.0), Some(BODY_CHILD)),
            )
        }))
        .on_toggle(|s: &mut S, index| s.toggled.push(index));
        let mut w = build(&view);
        layout(&mut w);
        let mut state = S::default();
        let y = w.geometry[1].y + header_height() / 2.0;
        dispatch(&mut w, &mut state, PointerPhase::Down, Point::new(10.0, y));
        dispatch(&mut w, &mut state, PointerPhase::Up, Point::new(10.0, y));
        assert_eq!(state.toggled, vec![1]);
        // Controlled: the widget's own set is untouched.
        assert!(w.expanded.is_empty());
    }

    #[test]
    fn a_release_outside_the_armed_header_fires_nothing() {
        #[derive(Default)]
        struct S {
            toggled: Vec<usize>,
        }
        let view: ExpandableListView<S> = expandable_list((0..2).map(|_| {
            expandable_item(
                frust::SizedBox::<S>(Some(40.0), Some(HEADER_CHILD)),
                frust::SizedBox::<S>(Some(40.0), Some(BODY_CHILD)),
            )
        }))
        .on_toggle(|s: &mut S, index| s.toggled.push(index));
        let mut w = build(&view);
        layout(&mut w);
        let mut state = S::default();
        let y0 = header_height() / 2.0;
        let y1 = w.geometry[1].y + header_height() / 2.0;
        dispatch(&mut w, &mut state, PointerPhase::Down, Point::new(10.0, y0));
        dispatch(&mut w, &mut state, PointerPhase::Up, Point::new(10.0, y1));
        assert!(state.toggled.is_empty());
    }

    #[test]
    fn a_cancel_clears_the_press_without_firing() {
        #[derive(Default)]
        struct S {
            toggled: Vec<usize>,
        }
        let view: ExpandableListView<S> = expandable_list((0..1).map(|_| {
            expandable_item(
                frust::SizedBox::<S>(Some(40.0), Some(HEADER_CHILD)),
                frust::SizedBox::<S>(Some(40.0), Some(BODY_CHILD)),
            )
        }))
        .on_toggle(|s: &mut S, index| s.toggled.push(index));
        let mut w = build(&view);
        layout(&mut w);
        let mut state = S::default();
        let y = header_height() / 2.0;
        dispatch(&mut w, &mut state, PointerPhase::Down, Point::new(10.0, y));
        dispatch(
            &mut w,
            &mut state,
            PointerPhase::Cancel,
            Point::new(10.0, y),
        );
        assert!(state.toggled.is_empty());
        assert!(w.armed.is_none());
        assert!(!w.state.pressed);
    }

    #[test]
    fn a_secondary_press_never_arms_the_header() {
        let view: ExpandableListView<()> = expandable_list(items(1)).on_toggle(|_: &mut (), _| {});
        let mut w = build(&view);
        layout(&mut w);
        let event = InputEvent::Pointer(frust::authoring::PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(10.0, header_height() / 2.0),
            button: frust::authoring::PointerButton::Secondary,
        });
        let mut state = ();
        let sa: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(WIDTH, 4000.0));
        w.event(&mut ctx, &event);
        assert!(w.armed.is_none());
    }

    #[test]
    fn a_press_in_a_revealed_body_never_arms_the_header() {
        let view: ExpandableListView<()> = expandable_list(items(1))
            .expanded([0])
            .on_toggle(|_: &mut (), _| {});
        let mut w = build(&view);
        layout(&mut w);
        let mut state = ();
        let in_body = header_height() + 10.0;
        dispatch(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(10.0, in_body),
        );
        assert!(w.armed.is_none());
    }

    #[test]
    fn a_stack_without_on_toggle_routes_pointers_to_its_children() {
        let view: ExpandableListView<()> = expandable_list(items(1));
        let mut w = build(&view);
        layout(&mut w);
        let mut state = ();
        dispatch(
            &mut w,
            &mut state,
            PointerPhase::Down,
            Point::new(10.0, header_height() / 2.0),
        );
        assert!(w.armed.is_none());
    }

    #[test]
    fn header_hit_testing_ignores_the_gap_between_items() {
        let view: ExpandableListView<()> = expandable_list(items(2));
        let mut w = build(&view);
        layout(&mut w);
        let gap_y = header_height() + EXPANDABLE_GAP / 2.0;
        assert_eq!(w.header_at(Point::new(10.0, gap_y)), None);
        assert_eq!(w.header_at(Point::new(10.0, 1.0)), Some(0));
        assert_eq!(
            w.header_at(Point::new(10.0, w.geometry[1].y + 1.0)),
            Some(1)
        );
    }

    // ---- Rebuild bookkeeping ---------------------------------------------

    #[test]
    fn a_structural_change_reseeds_the_reveal_drivers() {
        let two: ExpandableListView<()> = expandable_list(items(2)).expanded([1]);
        let mut w = build(&two);
        layout(&mut w);
        assert_eq!(w.reveals.len(), 2);
        let three: ExpandableListView<()> = expandable_list(items(3)).expanded([2]);
        let mut counter = 0u64;
        <ExpandableListView<()> as View<()>>::rebuild(
            &three,
            &two,
            &mut w,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(w.reveals.len(), 3);
        assert_eq!(w.reveals[2].value, 1.0, "the new set seeds at rest");
        assert_eq!(w.reveals[1].value, 0.0);
    }

    #[test]
    fn semantics_is_a_clickable_list_forwarding_every_pod() {
        fn logic(_s: &mut ()) -> ExpandableListView<()> {
            expandable_list([expandable_item(frust::text("header"), frust::text("body"))])
                .on_toggle(|_: &mut (), _| {})
        }
        let mut root: frust_core::RenderRoot<(), ExpandableListView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(WIDTH, 800.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("the stack contributes a Role::List node");
        assert!(
            node.supports_action(Action::Click),
            "an interactive stack carries Action::Click"
        );
        assert!(
            !node.children().is_empty(),
            "every header and body pod is a semantics child"
        );
    }
}
