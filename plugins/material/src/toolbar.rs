// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/toolbars/` — `m3e_toolbars.dart`,
// `m3e_toolbar_scroll_behavior.dart`, `controllers/m3e_toolbar_visibility_controller.dart`,
// `components/` (`m3e_toolbar_build.dart`, `m3e_toolbar_body.dart`,
// `m3e_toolbar_actions_row.dart`, `m3e_toolbar_fab_layout.dart`,
// `m3e_toolbar_fab_slot.dart`, `m3e_toolbar_overflow_menu.dart`,
// `m3e_toolbar_icon_button.dart`, `m3e_toolbar_measure_size.dart`,
// `m3e_toolbar_title_block.dart`, `m3e_toolbar_expanding_actions.dart`),
// `enums/m3e_toolbar_enums.dart`, `models/m3e_toolbar_item.dart`,
// `res/m3e_toolbar_tokens.dart`, `styles/m3e_toolbar_theme.dart`, and
// `utils/` (`m3e_toolbar_item_layout.dart`, `m3e_toolbar_spring_motion.dart`)
// (retrieved 2026-08-20). Upstream:
// https://github.com/paadevelopments/material_3_expressive
// Porting decisions (each restated with its reference site in the module docs
// below): expansion and scroll-hide are app-owned controlled props rather than
// the reference's internal `_expanded` mirror and ticker-driven visibility
// controller; the adjacent FAB is morphed through this widget's own tight
// constraints rather than the reference's `FittedBox` rescale; the overflow
// panel is a second, app-mounted view rather than something this widget hosts;
// and the vertical axis, the title/subtitle block and the labeled-action width
// spring stay unported.

//! The M3 Expressive **floating/docked toolbar**
//! (m3.material.io/components/toolbars/specs; Compose
//! `HorizontalFloatingToolbar`/`FlexibleBottomAppBar`).
//!
//! [`ToolbarView`]/[`ToolbarWidget`] mirror [`super::appbar`]'s slot
//! conventions (read that module first): **leading**, **center**, and
//! **trailing** slots, each an ordered `Vec` of opaque [`AnyView`] children
//! this widget lays out and routes events to but never paints or tints itself.
//! An optional **fab** slot (`.fab(...)`) is the adjacent prominent action.
//! Alongside the opaque slots sits a *typed* [`ToolbarAction`] list
//! ([`ToolbarView::actions`], upstream's own `actions`) — the only slot this
//! widget composes itself, because it is the one that has to survive the
//! overflow split below.
//!
//! # Variants ([`ToolbarVariant`]) and color styles ([`ToolbarColorStyle`])
//!
//! * **Floating**: self-sized (hugs its content), a fully-rounded pill
//!   (`shape.full`, resolved to a true pill via [`frust::ShapeScale::resolve`]
//!   against the bar's own fixed height). "Floating with inset margins above
//!   content" describes the *placement* a caller gives it (a [`frust::Stack`]
//!   with [`frust::Align`]/[`frust::Padding`] around this self-sized widget) —
//!   this widget implements no margin/anchoring logic of its own.
//! * **Docked**: fills the available width (`bc.max().width`, like
//!   [`super::appbar`]/[`super::navbar`]), flat geometry (`shape.none`) — the
//!   flat replacement for the static bottom-app-bar pattern. The leading group
//!   hugs the leading edge, the trailing group (+ the fab slot, if any) the
//!   trailing edge in reading order, and the center group is centered as a
//!   whole between them.
//!
//! `M3EToolbarTheme.colors` maps the two color styles onto four roles each:
//!
//! | Style | container | content | fab container | fab content |
//! |---|---|---|---|---|
//! | [`ToolbarColorStyle::Standard`] | `surface_container` | `on_surface` | `primary_container` | `on_primary_container` |
//! | [`ToolbarColorStyle::Vibrant`] | `primary_container` | `on_primary_container` | `tertiary_container` | `on_tertiary_container` |
//!
//! [`ToolbarColors::resolve`] is that table as a public function. Only
//! **container** is applied here; the other three are *advisory*, and for the
//! same reason: content is caller-owned. The fab slot is an opaque caller view
//! like every other slot, so a caller pairs a vibrant bar with
//! [`crate::FabColor::Tertiary`] (upstream's own
//! `style == vibrant ? M3EFabColor.tertiary : M3EFabColor.primary`) itself; and
//! `content` reaches upstream's own inline actions only through
//! `M3EToolbarTheme.scopedTheme`, a `Theme`-override push around the whole
//! content band that has no analogue on this framework's public surface — the
//! same ambient-tint gap [`mod@super::appbar`] documents for its own slots. A
//! vibrant bar's actions therefore keep [`mod@crate::icon_button`]'s own
//! `on_surface_variant` ink; an app that needs the vibrant pairing supplies its
//! own tinted views in the `center` slot instead of typed actions.
//!
//! # Metrics
//!
//! Every value is a `M3EToolbarTokens` constant:
//!
//! | Token | Value | Constant |
//! |---|---|---|
//! | `containerSize` | 64 | [`TOOLBAR_HEIGHT`] |
//! | `floatingContentPadding` | 8 | [`TOOLBAR_FLOATING_PAD`] |
//! | `dockedHorizontalPadding` | 16 | [`TOOLBAR_DOCKED_PAD_X`] |
//! | `containerBetweenSpace` | 4 | [`TOOLBAR_GAP`] |
//! | `toolbarToFabGap` | 8 | [`TOOLBAR_TO_FAB_GAP`] |
//! | `screenOffset` | 16 | [`TOOLBAR_SCREEN_OFFSET`] |
//! | `fabBaseline` | 56 | [`TOOLBAR_FAB_BASELINE`] |
//! | `fabMedium` | 80 | [`TOOLBAR_FAB_MEDIUM`] |
//! | `elevationNone` / `elevationWithFabExpanded` | 0 / 1 | [`ToolbarElevation`] |
//!
//! The elevation pair is a **behavior change** against this module's previous
//! shape, which pinned an M3 level-2 shadow under every floating pill: upstream
//! rests at `elevationNone` and only lifts to `elevationWithFabExpanded` when a
//! FAB is attached. [`ToolbarView::elevation`] reaches the old look explicitly
//! ([`ToolbarElevation::Level2`]) — upstream's own `elevation` override.
//!
//! # The FAB morph (80 → 56), and what the fab seam can carry
//!
//! With a FAB attached, a **floating** bar lays out as
//! `RenderM3EToolbarHorizontalFabLayout` does: a box
//! `pill + toolbarToFabGap + fabBaseline` wide and `fabMedium` (80) tall, the
//! pill vertically centered in it and the FAB pinned to
//! [`ToolbarFabPosition`]'s edge. The FAB's square is
//! `lerp(80, 56, progress)` — upstream's `_resolvedFabSize` — so it is **80dp
//! while the bar is collapsed and 56dp once expanded**, and the pill reveals
//! from the FAB's side across the same ramp (`toolbarWidth = natural *
//! progress`, clipped to the revealed window, faded through
//! `Interval(0.5, 1, easeIn)` — [`ToolbarMorph::reveal_alpha`]). `progress` is driven by
//! [`ToolbarView::expanded`] on
//! [`MaterialSpring::EXPRESSIVE_SPATIAL_FAST`](crate::MaterialSpring::EXPRESSIVE_SPATIAL_FAST)
//! ([`TOOLBAR_EXPAND_SPRING`]), upstream's own `m3eToolbarExpandMotion()`.
//! [`ToolbarView::fab_expands_toolbar`] pins the pill open and the FAB at 56
//! (upstream's `fabExpandsToolbar: false`).
//!
//! **Who owns the toggle**: upstream's FAB press flips an internal `_expanded`
//! mirror. The fab slot here is an opaque caller view holding its own press
//! handler, so expansion is a *controlled prop* instead — the same
//! never-self-mutating contract [`mod@crate::navigation_rail`] documents for its
//! own expansion. The app flips `expanded` from the FAB's `on_press`.
//!
//! **The fab-seam degrade, stated plainly**: upstream sizes its FAB by wrapping
//! it in a `FittedBox` — a *uniform rescale* of the whole button, container,
//! corner radius and icon alike. There is no rescale seam here, so the morph is
//! applied as **tight constraints** on the fab slot instead: the pod's laid-out
//! rect is the morphing square, which is what [`mod@crate::fab`] fills its
//! container, shadow and state layer against, and what hit-testing and the
//! accessibility bounds read. What tight constraints cannot reach is
//! [`mod@crate::fab`]'s *icon centering*, which is computed against its own
//! [`crate::FabSize`] tier (56dp for the default `Medium`) rather than the box
//! it is handed — so at the collapsed 80dp end the icon reads 12dp off-center.
//! Closing that needs either an edit to `fab.rs` (out of this module's scope)
//! or painting a child at a size it was not laid out at (which would desync
//! hit-testing from the visual). An app that wants a pixel-exact collapsed end
//! supplies its own fab view sized off [`ToolbarMorph::size`].
//!
//! # Scroll-hide: the app owns the wiring
//!
//! A widget here has no window or scroll handle, so — exactly as
//! [`super::appbar`]'s collapse contract documents — the hide is a **controlled
//! prop**: the app installs [`on_scroll`](frust::ScrollView::on_scroll) on its
//! own scroll view, decides visibility, and feeds it back down.
//!
//! ```
//! use frust::{ScrollInfo, scroll_view, text};
//! use frust_material::{ToolbarScrollHide, floating_toolbar};
//!
//! struct App {
//!     /// The last offset the body reported — the hide's delta source.
//!     scrolled: f64,
//!     /// Upstream's `M3EToolbarVisibilityController`, as a plain value.
//!     hide: ToolbarScrollHide,
//! }
//!
//! fn bar(state: &App) -> frust_material::ToolbarView<App> {
//!     floating_toolbar().visible(!state.hide.is_hidden())
//! }
//!
//! fn body(_state: &App) -> impl frust::View<App> + use<> {
//!     scroll_view(text("content")).on_scroll(|state: &mut App, info: ScrollInfo| {
//!         let delta = info.offset - state.scrolled;
//!         state.scrolled = info.offset;
//!         state.hide.scroll(delta);
//!     })
//! }
//! ```
//!
//! [`ToolbarScrollHide`] is upstream's `M3EToolbarVisibilityController` as a
//! pure value (its ticker belongs to the widget, not the controller): the same
//! `offset -= delta` accumulation clamped into `[offset_limit, 0]`, the same
//! `collapsedFraction`, the same `-(extent + screenOffset)` measured limit, and
//! the same `|velocity| > 150` settle rule ([`TOOLBAR_SETTLE_VELOCITY`]).
//!
//! Two props consume it, mirroring upstream's own two paths:
//!
//! * [`ToolbarView::visible`] — the animated `show()`/`hide()` path, sprung on
//!   [`TOOLBAR_EXPAND_SPRING`] (upstream's controller shares the expand
//!   motion).
//! * [`ToolbarView::hidden_fraction`] — the 1:1 drag path (upstream's plain
//!   `offset` setter during a scroll update): pinned every frame, no spring.
//!
//! The bar slides along [`ToolbarExitDirection`] by
//! `fraction × exit_extent`, clipped to its own box — upstream's
//! `ClipRect(Transform.translate(...))` pair. The extent is
//! [`ToolbarView::exit_extent`] or, unset, the measured
//! `cross extent + screenOffset` (`M3EToolbarMeasureSize`'s own rule).
//!
//! **Both the hide offset and the FAB square are layout values**, so an
//! in-flight ramp calls [`frust::authoring::PaintCtx::request_layout`] rather
//! than merely asking for another frame — the layout-skip class
//! [`mod@crate::navigation_rail`] documents at length, including its
//! settling-frame refinement (the frame that both stops animating *and* lands
//! on the target still needs the relayout). `Theme.motion.reduce_motion` snaps
//! instead, and a snap that actually moved a value requests exactly one
//! relayout.
//!
//! # Overflow
//!
//! `M3EToolbarItemLayout.partitionInline` keeps every non-action slot inline
//! and overflows only surplus actions: an expand-trigger action always stays
//! inline and *reserves* one slot (`actionBudget = maxInline - reservedTrigger`),
//! and the first `actionBudget` remaining actions stay inline in order — the
//! rest overflow. [`partition_actions`] is that rule as a pure function;
//! [`ToolbarView::max_inline_actions`] is `maxInlineActions` (default
//! [`TOOLBAR_MAX_INLINE_ACTIONS`], 4).
//!
//! When anything overflows, a trailing trigger ([`ToolbarView::overflow_icon`],
//! upstream's `more_vert`) joins the inline row and reports
//! [`ToolbarView::on_overflow`] on press. The hidden tail presents as an
//! anchored menu through [`ToolbarView::overflow_menu`] — the same two-piece
//! shape [`mod@crate::button_group`]'s `Popup` overflow establishes, and for the
//! same reason: [`mod@crate::overlay::anchored`] is a plain widget with no
//! navigator to reach out through, so the app mounts the panel itself (the top
//! of its own [`frust::Stack`]), anchored to the [`OverlayAnchor`] handed to
//! [`ToolbarView::overflow_anchor`], and toggles `open` rather than unmounting
//! it (the kept-mounted pattern). Activating a row runs that action's own
//! `on_press`, exactly as its inline button would.
//!
//! One additive divergence: upstream's overflow entry is label-only, while a
//! [`ToolbarAction`] always carries an [`frust::IconSource`], so the same glyph
//! the inline button shows leads the overflowed row.
//!
//! # Semantics
//!
//! The whole bar is one [`Role::Toolbar`] container node (the accesskit role
//! purpose-built for this UI region), optionally labelled with
//! [`ToolbarView::semantic_label`], whose children are every slot pod in visual
//! order — leading, center, inline actions, the overflow trigger, trailing,
//! then the fab slot — via
//! [`frust::authoring::SemanticsCtx::push_container`].
//!
//! # Not ported
//!
//! * **The vertical axis** (`VerticalFloatingToolbar`,
//!   `M3EToolbarVerticalFabLayout`) — a horizontal bar is the whole of this
//!   module's v1; the vertical layout is a second, independent geometry.
//! * **The title/subtitle block** (`M3EToolbarTitleBlock`, `centerTitle`) — a
//!   toolbar has no mandatory title, and the `center` slot already carries any
//!   caller view a title would be.
//! * **The labeled-action width spring** (`M3EToolbarIconButton`'s in-button
//!   label morph, `pillActiveSpring`, `reservedLabeledSelectionExtent`) —
//!   [`mod@crate::icon_button`] has no label slot to morph, so an active action
//!   changes variant only. A [`ToolbarAction::label`] is still the overflow
//!   row's title, its other upstream use.
//! * **`safeArea` / `MediaQuery.viewPadding`** — this catalog's chrome widgets
//!   never self-inset (`docs/CODE_STANDARDS.md`'s
//!   self-sizing-chrome-consumes-its-own-inset rule leaves the choice to the
//!   composer); a caller wraps the bar in `frust::safe_area(...)`.
//! * **`clipBehavior` / `foregroundColor` / `scopedTheme` / `M3EToolbarTheme`
//!   as a theme extension** — slot content is opaque here, there is no ambient
//!   `Theme`-override push to scope a foreground onto (see the color table
//!   above), and every color/shape below resolves from [`frust::Theme`]
//!   directly (MaterialTokens-only), an explicit builder value winning over
//!   both.
//! * **`ExcludeFocus` while hidden** — a fully-hidden bar's slots are
//!   translated outside its own box, so pointer input cannot reach them; a
//!   focus-routed key event still can, which an app avoids by not focusing a
//!   hidden bar.
//!
//! # Clamp discipline
//!
//! `f64::clamp` panics if `min > max` or either bound is NaN — a hard abort
//! under `panic = "abort"`, not a catchable error (see [`crate::slider`]'s own
//! core for the same rule stated at length, which this one mirrors). Two
//! caller-fed values are the module's non-finite entry points, and both are
//! funneled to a safe value right where they enter (the module's private
//! `finite_or_zero`) rather than trusted downstream:
//!
//! * [`ToolbarScrollHide::new`] (and [`ToolbarScrollHide::for_bar`], which
//!   delegates to it) funnels its `exit_extent`/`cross_extent` construction
//!   parameter through `finite_or_zero` before it can reach `limit` — an
//!   unguarded non-finite `limit` would abort `set_offset`'s clamp on the
//!   very next [`ToolbarScrollHide::scroll`].
//! * `ToolbarWidget::layout`'s `extent` (the resolved
//!   [`ToolbarView::exit_extent`] prop) is the same `finite_or_zero` funnel:
//!   unguarded, a non-finite `exit_extent` would carry through `shift` into
//!   `pill_origin`, which `reveal_window` then clamps *against* (`x0` is the
//!   second clamp's `min` bound) — a caller-fed extent reaching a clamp
//!   *bound*, not just a clamped value.
//!
//! That leaves two clamp shapes in the module tree:
//!
//! - **Bounds are literal `0.0`/`1.0` (or another literal constant)**
//!   (`clamp01`, `layout`'s `progress.clamp(0.0, REVEAL_OVERSHOOT)`) —
//!   trivially ordered, never a hazard.
//! - **Bounds are already-finite by construction, funneled at the one point
//!   caller input enters** (`set_offset`'s `self.limit`, `reveal_window`'s
//!   `box_size.width`/`x0`) — both trace back to a `finite_or_zero` funnel
//!   rather than a re-check at every site.
//!
//! A new clamp against a computed bound must fit one of these two shapes, not
//! introduce a third.
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section and its Module Attribution Header Convention.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, View, ViewSeq, Widget,
};
use frust::{
    AnimationController, Curve, FrameTime, IconSource, ShapeScale, SpringDesc, Theme, Tween, icon,
};
use kurbo::{Point, Size, Vec2};
use peniko::Color;

use crate::icon_button::{IconButtonSize, IconButtonVariant, icon_button};
use crate::menu::{MenuNode, MenuSelection, menu, menu_entry};
use crate::overlay::{OverlayAlign, OverlayAnchor, overlay_anchor};

// ---- Tokens (`res/m3e_toolbar_tokens.dart`'s `M3EToolbarTokens`) -----------

/// Cross-axis container size of both toolbar variants, in logical px —
/// `M3EToolbarTokens.containerSize`. The same M3 bar-height family
/// [`super::appbar`]'s `HEIGHT` and [`super::navbar`]'s `HEIGHT` share.
pub const TOOLBAR_HEIGHT: f64 = 64.0;

/// Content inset of the **floating** pill on every edge, in logical px —
/// `M3EToolbarTokens.floatingContentPadding` (`M3EToolbarTheme.metricsFor`
/// resolves it as `EdgeInsets.all(8)`).
pub const TOOLBAR_FLOATING_PAD: f64 = 8.0;

/// Horizontal content inset of the **docked** bar, in logical px —
/// `M3EToolbarTokens.dockedHorizontalPadding` (its vertical inset is
/// [`TOOLBAR_FLOATING_PAD`], which this fixed-height port has no use for).
pub const TOOLBAR_DOCKED_PAD_X: f64 = 16.0;

/// Gap between adjacent slot elements, in logical px —
/// `M3EToolbarTokens.containerBetweenSpace`, which
/// `M3EToolbarTheme.metricsFor` publishes as its `gap`.
pub const TOOLBAR_GAP: f64 = 4.0;

/// Gap between the pill and an adjacent FAB, in logical px —
/// `M3EToolbarTokens.toolbarToFabGap`.
pub const TOOLBAR_TO_FAB_GAP: f64 = 8.0;

/// Extra distance a scroll-hidden bar travels past its own extent, in logical
/// px — `M3EToolbarTokens.screenOffset`, the margin
/// `M3EToolbarMeasureSize`'s measured `offsetLimit` adds so the bar clears the
/// screen edge entirely.
pub const TOOLBAR_SCREEN_OFFSET: f64 = 16.0;

/// The adjacent FAB's **expanded** square, in logical px —
/// `M3EToolbarTokens.fabBaseline`. Also the width the pill's box always
/// reserves for the FAB, whatever the morph is currently painting.
pub const TOOLBAR_FAB_BASELINE: f64 = 56.0;

/// The adjacent FAB's **collapsed** square, in logical px —
/// `M3EToolbarTokens.fabMedium`, which is also the whole box's height whenever
/// a FAB is attached.
pub const TOOLBAR_FAB_MEDIUM: f64 = 80.0;

/// Default number of actions kept inline before the overflow split —
/// `M3EToolbar.maxInlineActions`'s own default.
pub const TOOLBAR_MAX_INLINE_ACTIONS: usize = 4;

/// The expand/hide spring — upstream's `m3eToolbarExpandMotion()`, i.e.
/// `M3EMotion.expressiveSpatialFast` (stiffness 800, damping ratio 0.6).
/// Restated as a [`SpringDesc`] rather than read from a theme for the reason
/// [`mod@crate::navigation_rail`]'s own springs document: a `fling` needs one, and
/// paint/event code reads no theme for motion. A test pins it to
/// [`MaterialSpring::EXPRESSIVE_SPATIAL_FAST`](crate::MaterialSpring::EXPRESSIVE_SPATIAL_FAST).
pub const TOOLBAR_EXPAND_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 800.0,
    damping_ratio: 0.6,
};

/// Fling velocity above which a scroll settle commits to the direction of
/// travel instead of the nearer end —
/// `M3EToolbarVisibilityController.settle`'s own `velocity.abs() > 150`.
pub const TOOLBAR_SETTLE_VELOCITY: f64 = 150.0;

/// Fraction of the expand ramp the revealing pill stays fully transparent for
/// — the start of upstream's `Interval(0.5, 1, curve: Curves.easeIn)`. See
/// [`ToolbarMorph::reveal_alpha`], which is that interval as a function
/// (`frust::Curve::interval`'s own `SegmentedCurve` return type is not
/// re-exported through the facade, so it cannot be named in a `const` here).
pub const TOOLBAR_REVEAL_FADE_START: f64 = 0.5;

/// Widest pill-reveal overshoot the layout honors — upstream's own
/// `_progress.clamp(0.0, 1.2)` on the revealed width, which lets the expand
/// spring's overshoot read as travel rather than being flattened.
const REVEAL_OVERSHOOT: f64 = 1.2;

/// Nominal period seeding a ramp [`AnimationController`]'s clock; both ramps
/// here are spring-driven via `fling`, so this backs construction only.
const RAMP_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity for a ramp leg. Sub-visible on purpose: a spring's shape
/// comes from its displacement, so only the sign matters, and it is always
/// positive here because a leg runs `0 → 1` in progress space — the same
/// sign-only convention [`mod@crate::navigation_rail`]'s own travel legs document.
const RAMP_LAUNCH_VELOCITY: f64 = 1e-3;

/// Progress difference under which a ramp counts as already heading somewhere
/// (a retarget guard, so an unchanged prop never relaunches a settled spring).
const RAMP_EPSILON: f64 = 1e-6;

// ---- Unthemed fallbacks ----------------------------------------------------

/// Unthemed fallback for `colors.surface_container` — matches
/// [`super::navbar`]'s own `CONTAINER` constant exactly (same M3 role).
const SURFACE_CONTAINER: Color = Color::from_rgb8(0xF3, 0xED, 0xF7);
/// Unthemed fallback for `colors.on_surface`.
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed fallback for `colors.primary_container` (matches [`mod@crate::fab`]'s).
const PRIMARY_CONTAINER: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);
/// Unthemed fallback for `colors.on_primary_container`.
const ON_PRIMARY_CONTAINER: Color = Color::from_rgb8(0x21, 0x00, 0x5D);
/// Unthemed fallback for `colors.tertiary_container`.
const TERTIARY_CONTAINER: Color = Color::from_rgb8(0xFF, 0xD8, 0xE4);
/// Unthemed fallback for `colors.on_tertiary_container`.
const ON_TERTIARY_CONTAINER: Color = Color::from_rgb8(0x31, 0x11, 0x1D);

/// Unthemed-fallback shadow y-offset at M3 elevation level 1, matching
/// `crate::tokens::elevation().level1`'s own `dp / 2.0 + 1.0` at `dp = 1.0`.
const LEVEL1_SHADOW_Y_OFFSET: f64 = 1.5;
/// Unthemed-fallback shadow blur std-dev at M3 elevation level 1.
const LEVEL1_SHADOW_BLUR: f64 = 1.0;
/// Unthemed-fallback shadow y-offset at M3 elevation level 2 (`dp = 3.0`).
const LEVEL2_SHADOW_Y_OFFSET: f64 = 2.5;
/// Unthemed-fallback shadow blur std-dev at M3 elevation level 2.
const LEVEL2_SHADOW_BLUR: f64 = 3.0;
/// Unthemed-fallback shadow color (opaque black at `crate::tokens::elevation()`'s
/// `0.3` alpha — identical at every level).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The accessible name of the overflow trigger — upstream's own
/// `tooltip: 'More options'`, overridable per bar with
/// [`ToolbarView::overflow_label`].
const OVERFLOW_LABEL: &str = "More options";

// ---- Enums -----------------------------------------------------------------

/// The M3X toolbar container variant — `M3EToolbarPlacement`. See the
/// [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarVariant {
    /// A self-sized, fully-rounded pill floating over content.
    #[default]
    Floating,
    /// A full-width, flat bar docked to an edge (bottom typical).
    Docked,
}

/// Standard vs vibrant container color mapping — `M3EToolbarColorStyle`. See
/// the [module docs](self)' color table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarColorStyle {
    /// `surface_container` container, `on_surface` content — the default.
    #[default]
    Standard,
    /// `primary_container` container, `on_primary_container` content.
    Vibrant,
}

/// The bar's resting elevation — upstream's `elevationNone` /
/// `elevationWithFabExpanded` pair, plus the level this module used to pin
/// unconditionally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarElevation {
    /// No shadow at all — `M3EToolbarTokens.elevationNone`, the resting
    /// default of a bar with no FAB.
    #[default]
    None,
    /// M3 elevation level 1 — `M3EToolbarTokens.elevationWithFabExpanded`, the
    /// resting default once a FAB is attached.
    Level1,
    /// M3 elevation level 2 — no upstream token; the level this module pinned
    /// under every floating pill before the reference's own pair was ported.
    Level2,
}

/// Which edge of the box an adjacent FAB occupies — `M3EToolbarFabPosition`'s
/// horizontal pair (its `top`/`bottom` belong to the unported vertical axis).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarFabPosition {
    /// The FAB leads the pill.
    Start,
    /// The FAB trails the pill — the default.
    #[default]
    End,
}

/// Direction a bar slides while scroll-hidden — `M3EToolbarExitDirection`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarExitDirection {
    /// Exits upwards.
    Top,
    /// Exits downwards — the default (a bottom-docked bar).
    #[default]
    Bottom,
    /// Exits toward the leading edge.
    Start,
    /// Exits toward the trailing edge.
    End,
}

impl ToolbarExitDirection {
    /// Whether this direction travels on the vertical axis, which is what
    /// picks the measured extent's own axis (`M3EToolbarMeasureSize`'s
    /// `vertical ? size.height : size.width`).
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }

    /// The translation a `distance` of travel produces — upstream's
    /// `_exitOffset`, restated for a non-negative distance (its own `offset`
    /// is `≤ 0`, so every arm there carries a sign flip this one does not).
    pub fn offset(self, distance: f64) -> Vec2 {
        match self {
            Self::Top => Vec2::new(0.0, -distance),
            Self::Bottom => Vec2::new(0.0, distance),
            Self::Start => Vec2::new(-distance, 0.0),
            Self::End => Vec2::new(distance, 0.0),
        }
    }
}

/// The icon-button density a bar renders its typed actions at — the legacy
/// `M3EToolbarSize`, which maps to icon-button density and never to container
/// height (`M3EToolbarTheme.iconButtonSize`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarSize {
    /// `IconButtonSize::Xs`.
    Small,
    /// `IconButtonSize::Sm` — the default.
    #[default]
    Medium,
    /// `IconButtonSize::Sm`, same as [`Self::Medium`] upstream.
    Large,
}

impl ToolbarSize {
    /// The icon-button tier this density resolves — `iconButtonSize`'s own
    /// `small → xs`, `medium | large → sm`.
    pub fn icon_button_size(self) -> IconButtonSize {
        match self {
            Self::Small => IconButtonSize::Xs,
            Self::Medium | Self::Large => IconButtonSize::Sm,
        }
    }
}

// ---- Colors ----------------------------------------------------------------

/// The four resolved roles of one [`ToolbarColorStyle`] —
/// `M3EToolbarColors`, as produced by `M3EToolbarTheme.colors`.
///
/// ```
/// use frust_material::{ToolbarColorStyle, ToolbarColors, baseline};
///
/// let theme = baseline();
/// let vibrant = ToolbarColors::resolve(Some(&theme), ToolbarColorStyle::Vibrant);
/// assert_eq!(vibrant.container, theme.scheme().primary_container);
/// assert_eq!(vibrant.fab_container, theme.scheme().tertiary_container);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarColors {
    /// The bar container's fill.
    pub container: Color,
    /// The ink every content slot the bar owns is tinted with.
    pub content: Color,
    /// The container an adjacent FAB should take — advisory here (the fab slot
    /// is an opaque caller view; see the [module docs](self)).
    pub fab_container: Color,
    /// The ink an adjacent FAB should take — advisory, as above.
    pub fab_content: Color,
}

impl ToolbarColors {
    /// Resolve `style` against `theme`, falling back to the M3 baseline
    /// literals when no theme is threaded. See the [module docs](self)' table.
    pub fn resolve(theme: Option<&Theme>, style: ToolbarColorStyle) -> Self {
        match (theme, style) {
            (Some(theme), ToolbarColorStyle::Standard) => {
                let s = theme.scheme();
                Self {
                    container: s.surface_container,
                    content: s.on_surface,
                    fab_container: s.primary_container,
                    fab_content: s.on_primary_container,
                }
            }
            (Some(theme), ToolbarColorStyle::Vibrant) => {
                let s = theme.scheme();
                Self {
                    container: s.primary_container,
                    content: s.on_primary_container,
                    fab_container: s.tertiary_container,
                    fab_content: s.on_tertiary_container,
                }
            }
            (None, ToolbarColorStyle::Standard) => Self {
                container: SURFACE_CONTAINER,
                content: ON_SURFACE,
                fab_container: PRIMARY_CONTAINER,
                fab_content: ON_PRIMARY_CONTAINER,
            },
            (None, ToolbarColorStyle::Vibrant) => Self {
                container: PRIMARY_CONTAINER,
                content: ON_PRIMARY_CONTAINER,
                fab_container: TERTIARY_CONTAINER,
                fab_content: ON_TERTIARY_CONTAINER,
            },
        }
    }
}

/// `value` clamped into `[0, 1]`, mapping NaN to the resting `0.0` rather than
/// propagating it into a layout (the same guard [`super::appbar`] applies).
fn clamp01(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// Linear interpolation from `a` to `b` at `t` (deliberately unclamped — the
/// FAB square reads the expand spring's overshoot as real travel, exactly as
/// upstream's own `_resolvedFabSize` does).
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// `constraint` if finite, `0.0` otherwise — the degenerate unbounded-width
/// case every full-width bar in this catalog resolves the same way.
fn finite_or_zero(constraint: f64) -> f64 {
    if constraint.is_finite() {
        constraint
    } else {
        0.0
    }
}

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved corner radius for `variant`, against a bar of `width`×`height`.
/// Themed: `shape.full` (floating, resolved to a true pill) / `shape.none`
/// (docked). Unthemed: the equivalent literal tokens
/// (`crate::tokens::shape_scale`'s own values), resolved the same way.
fn resolve_radius(theme: Option<&Theme>, variant: ToolbarVariant, width: f64, height: f64) -> f64 {
    let token = match (theme, variant) {
        (Some(theme), ToolbarVariant::Floating) => theme.shape.full,
        (Some(theme), ToolbarVariant::Docked) => theme.shape.none,
        (None, ToolbarVariant::Floating) => f64::INFINITY,
        (None, ToolbarVariant::Docked) => 0.0,
    };
    ShapeScale::resolve(token, width, height)
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters for
/// `level`, or `None` at [`ToolbarElevation::None`] (upstream's
/// `elevationNone`, which paints nothing at all). Themed: the matching
/// `theme.elevation` `ShadowSpec`, colored by `colors.shadow` at its
/// `color_alpha`. Unthemed: the matching `LEVEL{1,2}_SHADOW_*` constants.
fn resolve_shadow(theme: Option<&Theme>, level: ToolbarElevation) -> Option<(f64, f64, Color)> {
    match (theme, level) {
        (_, ToolbarElevation::None) => None,
        (Some(theme), level) => {
            let spec = match level {
                ToolbarElevation::Level1 => theme.elevation.level1,
                _ => theme.elevation.level2,
            };
            let shadow = spec.shadow(theme.brightness);
            let color = with_alpha(theme.scheme().shadow, shadow.color_alpha);
            Some((shadow.blur_std_dev, shadow.y_offset, color))
        }
        (None, ToolbarElevation::Level1) => Some((
            LEVEL1_SHADOW_BLUR,
            LEVEL1_SHADOW_Y_OFFSET,
            FALLBACK_SHADOW_COLOR,
        )),
        (None, ToolbarElevation::Level2) => Some((
            LEVEL2_SHADOW_BLUR,
            LEVEL2_SHADOW_Y_OFFSET,
            FALLBACK_SHADOW_COLOR,
        )),
    }
}

// ---- The expand-ramp geometry (pure) ---------------------------------------

/// The expand ramp's pure geometry: what the bar's expand progress does to the
/// adjacent FAB's square and to the revealing pill's opacity — an uninhabited
/// namespace type (no instance is ever constructed), the same shape
/// [`crate::MaterialSpring`]'s own token namespace uses.
///
/// ```
/// use frust_material::ToolbarMorph;
///
/// // Collapsed is the 80dp end; expanded the 56dp one.
/// assert_eq!(ToolbarMorph::size(0.0), 80.0);
/// assert_eq!(ToolbarMorph::size(1.0), 56.0);
/// assert_eq!(ToolbarMorph::size(0.5), 68.0);
///
/// // The spring's overshoot past 1 is real travel, never clamped away.
/// assert!(ToolbarMorph::size(1.1) < 56.0);
/// // …but never negative, whatever the input.
/// assert_eq!(ToolbarMorph::size(f64::NAN), 80.0);
///
/// // The pill stays invisible through the ramp's first half.
/// assert_eq!(ToolbarMorph::reveal_alpha(0.5), 0.0);
/// assert_eq!(ToolbarMorph::reveal_alpha(1.0), 1.0);
/// ```
#[derive(Debug)]
pub enum ToolbarMorph {}

impl ToolbarMorph {
    /// The FAB's square at expand `progress` — `_resolvedFabSize`'s own
    /// `fabMedium + (fabBaseline - fabMedium) * progress`, floored at zero and
    /// NaN-mapped to the collapsed (resting) end.
    pub fn size(progress: f64) -> f64 {
        if progress.is_nan() {
            return TOOLBAR_FAB_MEDIUM;
        }
        lerp(TOOLBAR_FAB_MEDIUM, TOOLBAR_FAB_BASELINE, progress).max(0.0)
    }

    /// The revealing pill's opacity at expand `progress` — upstream's
    /// `Interval(0.5, 1, curve: Curves.easeIn)`, whose control points
    /// ([`Curve::EaseIn`], `cubic-bezier(0.42, 0, 1, 1)`) are Flutter's
    /// `Curves.easeIn` exactly. See [`TOOLBAR_REVEAL_FADE_START`].
    pub fn reveal_alpha(progress: f64) -> f64 {
        Curve::EaseIn
            .interval(TOOLBAR_REVEAL_FADE_START, 1.0)
            .transform(clamp01(progress))
    }
}

// ---- Scroll-hide (pure) ----------------------------------------------------

/// Upstream's `M3EToolbarVisibilityController` as a pure value: the scroll
/// accumulation, clamp, and settle rule an app runs from its own
/// [`on_scroll`](frust::ScrollView::on_scroll), with the ticker left to the
/// widget (see the [module docs](self)' scroll-hide section).
///
/// `offset` is always `≤ 0` and clamped into `[offset_limit, 0]`.
///
/// ```
/// use frust_material::{TOOLBAR_HEIGHT, ToolbarScrollHide, TOOLBAR_SCREEN_OFFSET};
///
/// // The measured limit is `-(extent + screenOffset)`.
/// let mut hide = ToolbarScrollHide::for_bar(TOOLBAR_HEIGHT);
/// assert_eq!(hide.offset_limit(), -(TOOLBAR_HEIGHT + TOOLBAR_SCREEN_OFFSET));
///
/// // Scrolling down hides; scrolling back up returns.
/// hide.scroll(40.0);
/// assert_eq!(hide.offset(), -40.0);
/// hide.scroll(-15.0);
/// assert_eq!(hide.offset(), -25.0);
///
/// // A fling past the velocity threshold commits to its direction.
/// assert!(!hide.settle(400.0));
/// assert!(hide.is_hidden());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarScrollHide {
    limit: f64,
    offset: f64,
}

impl ToolbarScrollHide {
    /// A controller with a fixed exit distance — upstream's `exitExtent`,
    /// which pins `offsetLimit` to `-|extent|`. A non-finite `exit_extent`
    /// (NaN or ±∞) degrades to a resting `0.0` exit distance rather than
    /// carrying a non-finite `limit` into [`Self::set_offset`]'s clamp — see
    /// the [module docs](self)' clamp discipline.
    pub fn new(exit_extent: f64) -> Self {
        Self {
            limit: -finite_or_zero(exit_extent).abs(),
            offset: 0.0,
        }
    }

    /// A controller whose exit distance is measured off the bar — upstream's
    /// `M3EToolbarMeasureSize` rule, `-(cross_extent + screenOffset)`. A
    /// non-finite `cross_extent` degrades the same way [`Self::new`]'s own
    /// guard does — this delegates to it, so the guard applies whether the
    /// non-finite value reaches `new` directly or through here.
    pub fn for_bar(cross_extent: f64) -> Self {
        Self::new(cross_extent.abs() + TOOLBAR_SCREEN_OFFSET)
    }

    /// The current translation, in `[offset_limit, 0]`.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The most negative translation this controller allows.
    pub fn offset_limit(&self) -> f64 {
        self.limit
    }

    /// Consume a scroll `delta` (positive scrolling down) — upstream's
    /// `_updateOffset`'s `offset -= delta`, clamped. A non-finite delta is
    /// ignored rather than propagated into the offset.
    pub fn scroll(&mut self, delta: f64) {
        if !delta.is_finite() {
            return;
        }
        self.set_offset(self.offset - delta);
    }

    /// How far hidden the bar is, `0.0` fully visible through `1.0` fully
    /// hidden — upstream's `collapsedFraction`.
    pub fn collapsed_fraction(&self) -> f64 {
        if self.limit == 0.0 {
            return 0.0;
        }
        clamp01(self.offset / self.limit)
    }

    /// Whether the bar is fully off-screen — upstream's `isHidden`.
    pub fn is_hidden(&self) -> bool {
        self.collapsed_fraction() >= 1.0
    }

    /// Snap fully visible — upstream's `show()` target.
    pub fn show(&mut self) {
        self.offset = 0.0;
    }

    /// Snap fully hidden — upstream's `hide()` target.
    pub fn hide(&mut self) {
        self.offset = self.limit;
    }

    /// Flip about the halfway point — upstream's `toggle()`.
    pub fn toggle(&mut self) {
        if self.collapsed_fraction() < 0.5 {
            self.hide();
        } else {
            self.show();
        }
    }

    /// Settle after a fling at `velocity` (positive scrolling down), reporting
    /// whether the bar ends up **visible** — upstream's `settle`: past
    /// [`TOOLBAR_SETTLE_VELOCITY`] the direction of travel wins, otherwise the
    /// nearer end does. Already-settled at either end, nothing moves.
    pub fn settle(&mut self, velocity: f64) -> bool {
        if self.offset == 0.0 || self.offset == self.limit {
            return self.offset == 0.0;
        }
        let hide = if velocity.abs() > TOOLBAR_SETTLE_VELOCITY {
            velocity > 0.0
        } else {
            self.collapsed_fraction() >= 0.5
        };
        if hide {
            self.hide();
        } else {
            self.show();
        }
        !hide
    }

    /// Clamp and store `value` — upstream's `offset` setter, whose clamp
    /// bounds swap when the limit is (degenerately) positive.
    fn set_offset(&mut self, value: f64) {
        self.offset = if self.limit <= 0.0 {
            value.clamp(self.limit, 0.0)
        } else {
            value.clamp(0.0, self.limit)
        };
    }
}

// ---- The overflow capacity rule (pure) -------------------------------------

/// The inline/overflow split of one action list —
/// `M3EToolbarItemLayout.partitionInline`'s own result shape, as flat index
/// lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolbarPartition {
    /// Indices kept in the bar, in order.
    pub inline: Vec<usize>,
    /// Indices moved to the overflow menu, in order.
    pub overflow: Vec<usize>,
}

/// Split `count` actions into the inline and overflow halves —
/// `M3EToolbarItemLayout.partitionInline`.
///
/// An expand-trigger action (`trigger`) always stays inline **and** reserves
/// one of `max_inline`'s slots (upstream's `reservedTrigger`), so the budget
/// available to ordinary actions is `max_inline - 1` when one is present.
///
/// ```
/// use frust_material::{TOOLBAR_MAX_INLINE_ACTIONS, partition_actions};
///
/// // Six actions at the default budget: four inline, two overflowed.
/// let split = partition_actions(6, TOOLBAR_MAX_INLINE_ACTIONS, None);
/// assert_eq!(split.inline, vec![0, 1, 2, 3]);
/// assert_eq!(split.overflow, vec![4, 5]);
///
/// // A trigger stays inline and costs one budget slot.
/// let split = partition_actions(6, TOOLBAR_MAX_INLINE_ACTIONS, Some(5));
/// assert_eq!(split.inline, vec![0, 1, 2, 5]);
/// assert_eq!(split.overflow, vec![3, 4]);
/// ```
pub fn partition_actions(
    count: usize,
    max_inline: usize,
    trigger: Option<usize>,
) -> ToolbarPartition {
    let trigger = trigger.filter(|index| *index < count);
    let budget = max_inline.saturating_sub(usize::from(trigger.is_some()));
    let mut partition = ToolbarPartition::default();
    let mut used = 0usize;
    for index in 0..count {
        if Some(index) == trigger {
            partition.inline.push(index);
        } else if used < budget {
            partition.inline.push(index);
            used += 1;
        } else {
            partition.overflow.push(index);
        }
    }
    partition
}

// ---- Typed actions ---------------------------------------------------------

/// A view-held, typed press callback (erased per-action on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// One typed action in [`ToolbarView::actions`] — `M3EToolbarAction`, the
/// upstream item kind that can move to the overflow menu (its
/// `M3EToolbarWidget` sibling, which always stays inline, is this port's
/// opaque `center` slot).
pub struct ToolbarAction<State: 'static> {
    icon: IconSource,
    label: Option<String>,
    tooltip: Option<String>,
    semantic_label: Option<String>,
    enabled: bool,
    destructive: bool,
    active: bool,
    expand_trigger: bool,
    on_press: OnPress<State>,
}

/// Create a toolbar action painting `icon`, running `on_press` on release
/// inside its bounds.
pub fn toolbar_action<State: 'static, F: Fn(&mut State) + 'static>(
    icon: IconSource,
    on_press: F,
) -> ToolbarAction<State> {
    ToolbarAction {
        icon,
        label: None,
        tooltip: None,
        semantic_label: None,
        enabled: true,
        destructive: false,
        active: false,
        expand_trigger: false,
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> ToolbarAction<State> {
    /// The action's label — upstream's `label`. Used as the overflow row's
    /// title; the in-button label morph it also drives upstream is unported
    /// (see the [module docs](self)' *Not ported*).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The action's tooltip text — upstream's `tooltip`. This catalog's icon
    /// button carries no tooltip slot, so this contributes an accessible name
    /// (behind [`Self::semantic_label`]) and an overflow-row title (behind
    /// [`Self::label`]) only.
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// The action's accessible name — upstream's `semanticLabel`.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// Whether the action accepts presses (`enabled`, default `true`).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Whether the overflow row reads destructive (`isDestructive`).
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Whether the inline button reads selected (`active`) — filled icon-button
    /// styling, per `M3EToolbarIconButton._resolvedVariant`.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Mark this action as the bar's expand/collapse trigger
    /// (`isExpandTrigger`): it always stays inline, reserves one inline slot,
    /// and takes the same filled styling an active action does. At most one
    /// action should set it — the first one wins.
    ///
    /// The toggle itself stays app-owned (see the [module docs](self)'
    /// controlled-prop note): a trigger's own `on_press` is where an app flips
    /// [`ToolbarView::expanded`].
    pub fn expand_trigger(mut self, trigger: bool) -> Self {
        self.expand_trigger = trigger;
        self
    }

    /// The title this action's overflow row takes — upstream's
    /// `label ?? tooltip ?? semanticLabel ?? 'Action {n}'` fallback chain.
    pub fn menu_label(&self, index: usize) -> String {
        self.label
            .as_deref()
            .or(self.tooltip.as_deref())
            .or(self.semantic_label.as_deref())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Action {}", index + 1))
    }

    /// The inline icon-button view for this action.
    fn inline_view(&self, size: ToolbarSize) -> AnyView<State> {
        let on_press = self.on_press.clone();
        let mut button = icon_button::<State, _>(
            frust::authoring::any::<State, _>(icon(self.icon)),
            move |state: &mut State| on_press(state),
        )
        .size(size.icon_button_size())
        .enabled(self.enabled)
        .variant(if self.active || self.expand_trigger {
            IconButtonVariant::Filled
        } else {
            IconButtonVariant::Standard
        });
        if let Some(label) = self.semantic_label.as_deref().or(self.tooltip.as_deref()) {
            button = button.semantic_label(label);
        }
        frust::authoring::any::<State, _>(button)
    }
}

// ---- Motion driver ---------------------------------------------------------

/// One spring-driven `0 → 1` ramp (the expand progress, the hide fraction).
struct RampTrack {
    /// The progress `layout` reads this frame — **unclamped**, so the spring's
    /// overshoot is real travel; every consumer clamps at its own use site
    /// exactly as upstream does.
    value: f64,
    /// The progress the current leg is heading for.
    target: f64,
    /// Maps the controller's `0 → 1` leg onto `from → target`.
    tween: Tween<f64>,
    driver: AnimationController,
}

impl RampTrack {
    /// A ramp resting at `value` with no leg in flight — a bar built already
    /// expanded shows its full pill on the first frame rather than animating
    /// into it.
    fn resting(value: f64) -> Self {
        Self {
            value,
            target: value,
            tween: Tween::new(value, value),
            driver: AnimationController::new(RAMP_PERIOD),
        }
    }

    /// Launch a leg toward `target` on [`TOOLBAR_EXPAND_SPRING`], starting
    /// from wherever the ramp currently reads (so an interrupted leg reverses
    /// smoothly). A no-op if already heading there.
    fn set_target(&mut self, target: f64) {
        if (self.target - target).abs() < RAMP_EPSILON {
            return;
        }
        self.target = target;
        self.tween = Tween::new(self.value, target);
        self.driver = AnimationController::new(RAMP_PERIOD);
        self.driver
            .fling(RAMP_LAUNCH_VELOCITY, TOOLBAR_EXPAND_SPRING);
    }

    /// Jump to `to`, cancelling any leg (a prop-pinned fraction, or a fresh
    /// build).
    fn jump(&mut self, to: f64) {
        self.value = to;
        self.target = to;
        self.tween = Tween::new(to, to);
        self.driver.stop();
    }

    /// Jump to the target, cancelling any leg (the reduce-motion path).
    /// Returns whether that actually moved the value.
    fn snap(&mut self) -> bool {
        let moved = self.value != self.target;
        self.jump(self.target);
        moved
    }

    /// Advance one frame, returning whether the leg is still in flight.
    fn advance(&mut self, now: FrameTime) -> bool {
        let animating = self.driver.advance(now);
        self.value = self.tween.lerp(self.driver.value());
        animating
    }
}

// ---- The view --------------------------------------------------------------

/// A view-held, typed overflow-trigger callback.
type OnOverflow<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3X toolbar. See the [module docs](self).
pub struct ToolbarView<State: 'static> {
    variant: ToolbarVariant,
    color_style: ToolbarColorStyle,
    elevation: Option<ToolbarElevation>,
    size: ToolbarSize,
    leading: Vec<AnyView<State>>,
    center: Vec<AnyView<State>>,
    trailing: Vec<AnyView<State>>,
    fab: Option<AnyView<State>>,
    actions: Vec<ToolbarAction<State>>,
    max_inline_actions: usize,
    overflow_icon: IconSource,
    overflow_label: String,
    on_overflow: Option<OnOverflow<State>>,
    overflow_anchor: Option<OverlayAnchor>,
    expanded: bool,
    fab_expands_toolbar: bool,
    fab_position: ToolbarFabPosition,
    visible: bool,
    hidden_fraction: Option<f64>,
    exit_direction: ToolbarExitDirection,
    exit_extent: Option<f64>,
    semantic_label: Option<String>,
}

/// Build a [`ToolbarView`] of `variant` with every prop at its upstream
/// default.
fn toolbar<State: 'static>(variant: ToolbarVariant) -> ToolbarView<State> {
    ToolbarView {
        variant,
        color_style: ToolbarColorStyle::default(),
        elevation: None,
        size: ToolbarSize::default(),
        leading: Vec::new(),
        center: Vec::new(),
        trailing: Vec::new(),
        fab: None,
        actions: Vec::new(),
        max_inline_actions: TOOLBAR_MAX_INLINE_ACTIONS,
        overflow_icon: crate::icons::MORE_VERT,
        overflow_label: OVERFLOW_LABEL.to_owned(),
        on_overflow: None,
        overflow_anchor: None,
        expanded: true,
        fab_expands_toolbar: true,
        fab_position: ToolbarFabPosition::default(),
        visible: true,
        hidden_fraction: None,
        exit_direction: ToolbarExitDirection::default(),
        exit_extent: None,
        semantic_label: None,
    }
}

/// Create a floating toolbar (self-sized pill, elevated above content) with
/// no slots attached — attach them with [`ToolbarView::leading`]/
/// [`ToolbarView::center`]/[`ToolbarView::trailing`]/[`ToolbarView::actions`]/
/// [`ToolbarView::fab`].
pub fn floating_toolbar<State: 'static>() -> ToolbarView<State> {
    toolbar(ToolbarVariant::Floating)
}

/// PascalCase alias for [`floating_toolbar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn FloatingToolbar<State: 'static>() -> ToolbarView<State> {
    floating_toolbar()
}

/// Create a docked toolbar (full-width, flat bar) with no slots attached.
pub fn docked_toolbar<State: 'static>() -> ToolbarView<State> {
    toolbar(ToolbarVariant::Docked)
}

/// PascalCase alias for [`docked_toolbar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn DockedToolbar<State: 'static>() -> ToolbarView<State> {
    docked_toolbar()
}

impl<State: 'static> ToolbarView<State> {
    /// Attach leading slot children, in reading order (hugs the leading edge).
    /// Tint/styling is each supplied view's own responsibility.
    pub fn leading<M>(mut self, leading: impl ViewSeq<State, M>) -> Self {
        self.leading.clear();
        leading.extend_views(&mut self.leading);
        self
    }

    /// Attach center slot children, in reading order — upstream's inline
    /// `M3EToolbarWidget` items, which never overflow. They lead the typed
    /// [`Self::actions`] within the same center group.
    pub fn center<M>(mut self, center: impl ViewSeq<State, M>) -> Self {
        self.center.clear();
        center.extend_views(&mut self.center);
        self
    }

    /// Attach trailing slot children, in reading order (hugs the trailing
    /// edge, or the fab slot if also attached).
    pub fn trailing<M>(mut self, trailing: impl ViewSeq<State, M>) -> Self {
        self.trailing.clear();
        trailing.extend_views(&mut self.trailing);
        self
    }

    /// Attach the optional adjacent FAB — upstream's `floatingActionButton`.
    /// On a **floating** bar this takes the adjacent morph layout described in
    /// the [module docs](self); on a **docked** one it simply trails the row,
    /// as it always has (upstream has no docked FAB at all).
    pub fn fab(mut self, fab: impl View<State>) -> Self {
        self.fab = Some(AnyView::new(fab));
        self
    }

    /// Attach the typed action list — upstream's `actions`, the only slot
    /// subject to the overflow split.
    pub fn actions(mut self, actions: Vec<ToolbarAction<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// How many actions stay inline before overflowing (`maxInlineActions`,
    /// default [`TOOLBAR_MAX_INLINE_ACTIONS`]).
    pub fn max_inline_actions(mut self, max: usize) -> Self {
        self.max_inline_actions = max;
        self
    }

    /// Replace the overflow trigger's glyph (`overflowIcon`, default
    /// `more_vert`).
    pub fn overflow_icon(mut self, icon: IconSource) -> Self {
        self.overflow_icon = icon;
        self
    }

    /// Replace the overflow trigger's accessible name (upstream's own
    /// `'More options'`).
    pub fn overflow_label(mut self, label: impl Into<String>) -> Self {
        self.overflow_label = label.into();
        self
    }

    /// Fired when the overflow trigger is pressed — where an app flips the
    /// open flag it hands [`Self::overflow_menu`].
    pub fn on_overflow<F: Fn(&mut State) + 'static>(mut self, on_overflow: F) -> Self {
        self.on_overflow = Some(Rc::new(on_overflow));
        self
    }

    /// Capture the overflow trigger's window rect into `anchor` on every
    /// paint, so [`Self::overflow_menu`] has a rect to place against — the
    /// same [`OverlayAnchor`] handoff [`mod@crate::button_group`]'s own popup
    /// overflow takes.
    pub fn overflow_anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.overflow_anchor = Some(anchor.clone());
        self
    }

    /// Pick the standard or vibrant color resolution (`colorStyle`).
    pub fn color_style(mut self, style: ToolbarColorStyle) -> Self {
        self.color_style = style;
        self
    }

    /// Override the resting elevation (upstream's `elevation`). Unset, a bar
    /// rests at [`ToolbarElevation::None`], or [`ToolbarElevation::Level1`]
    /// once a FAB is attached.
    pub fn elevation(mut self, elevation: ToolbarElevation) -> Self {
        self.elevation = Some(elevation);
        self
    }

    /// The icon-button density typed actions render at (`size`).
    pub fn size(mut self, size: ToolbarSize) -> Self {
        self.size = size;
        self
    }

    /// Whether the pill is expanded (`expanded`, default `true`) — the
    /// controlled prop driving the FAB morph and pill reveal. See the
    /// [module docs](self) for who owns the toggle.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Whether an adjacent FAB morphs with the pill (`fabExpandsToolbar`,
    /// default `true`). Set `false` to pin the pill open and the FAB at
    /// [`TOOLBAR_FAB_BASELINE`].
    pub fn fab_expands_toolbar(mut self, expands: bool) -> Self {
        self.fab_expands_toolbar = expands;
        self
    }

    /// Which edge an adjacent FAB occupies (`fabPosition`, default
    /// [`ToolbarFabPosition::End`]).
    pub fn fab_position(mut self, position: ToolbarFabPosition) -> Self {
        self.fab_position = position;
        self
    }

    /// Whether the bar is shown (`show()`/`hide()`, default `true`) — sprung
    /// on [`TOOLBAR_EXPAND_SPRING`]. Ignored while [`Self::hidden_fraction`]
    /// pins a value.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Pin the hidden fraction directly, `0.0` visible through `1.0` hidden —
    /// upstream's plain `offset` setter during a scroll drag. `Some(f)`
    /// overrides [`Self::visible`] and applies with no spring; `None` (the
    /// default) hands the ramp back to `visible`, resuming from wherever the
    /// pin left it.
    pub fn hidden_fraction(mut self, fraction: Option<f64>) -> Self {
        self.hidden_fraction = fraction;
        self
    }

    /// The direction the bar slides while hidden (`exitDirection`, default
    /// [`ToolbarExitDirection::Bottom`]).
    pub fn exit_direction(mut self, direction: ToolbarExitDirection) -> Self {
        self.exit_direction = direction;
        self
    }

    /// Override the hide travel distance (`exitExtent`). Unset, the bar
    /// measures its own cross extent and adds [`TOOLBAR_SCREEN_OFFSET`].
    pub fn exit_extent(mut self, extent: Option<f64>) -> Self {
        self.exit_extent = extent;
        self
    }

    /// The accessible name of the bar as a whole (`semanticLabel`).
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// The index of the expand-trigger action, if any (the first one wins —
    /// upstream asserts at most one).
    fn trigger_index(&self) -> Option<usize> {
        self.actions.iter().position(|a| a.expand_trigger)
    }

    /// This bar's inline/overflow split (see [`partition_actions`]).
    fn partition(&self) -> ToolbarPartition {
        partition_actions(
            self.actions.len(),
            self.max_inline_actions,
            self.trigger_index(),
        )
    }

    /// The overflowed rows, as `(node, target)` pairs: the menu row to render
    /// and the action callback activating it runs.
    fn overflow_entries(&self) -> Vec<(MenuNode, OnPress<State>)> {
        self.partition()
            .overflow
            .into_iter()
            .map(|index| {
                let action = &self.actions[index];
                let node: MenuNode = menu_entry(action.menu_label(index))
                    .leading(action.icon)
                    .enabled(action.enabled)
                    .destructive(action.destructive)
                    .into();
                (node, action.on_press.clone())
            })
            .collect()
    }

    /// The composed views for the typed actions plus, when anything
    /// overflowed, the trailing overflow trigger — the center group's own
    /// tail.
    fn action_views(&self) -> Vec<AnyView<State>> {
        let split = self.partition();
        let mut views: Vec<AnyView<State>> = split
            .inline
            .iter()
            .map(|index| self.actions[*index].inline_view(self.size))
            .collect();
        if !split.overflow.is_empty() {
            views.push(self.trigger_view());
        }
        views
    }

    /// The overflow trigger's own view — an icon button reporting
    /// [`Self::on_overflow`], wrapped in [`overlay_anchor`] when an anchor is
    /// wired so its window rect reaches [`Self::overflow_menu`].
    fn trigger_view(&self) -> AnyView<State> {
        let on_overflow = self.on_overflow.clone();
        let trigger = icon_button::<State, _>(
            frust::authoring::any::<State, _>(icon(self.overflow_icon)),
            move |state: &mut State| {
                if let Some(callback) = &on_overflow {
                    callback(state);
                }
            },
        )
        .size(self.size.icon_button_size())
        .semantic_label(self.overflow_label.clone());
        match &self.overflow_anchor {
            Some(anchor) => frust::authoring::any::<State, _>(overlay_anchor(anchor, trigger)),
            None => frust::authoring::any::<State, _>(trigger),
        }
    }

    /// The anchored overflow panel for the actions that did not fit — mount
    /// this separately as the top of the app's own [`frust::Stack`], anchored
    /// to the same [`OverlayAnchor`] handed to [`Self::overflow_anchor`], and
    /// toggle `open` rather than unmounting it (the kept-mounted pattern
    /// [`mod@crate::overlay::anchored`] documents).
    ///
    /// Each overflowed action becomes a [`crate::menu::MenuEntry`] row led by
    /// its own glyph; activating one runs that action's `on_press`, exactly as
    /// its inline button would. The placement is upstream's own
    /// `M3EMenuAnchorPosition.bottomEnd`.
    pub fn overflow_menu(&self, open: bool) -> AnyView<State> {
        let anchor = self.overflow_anchor.clone().unwrap_or_default();
        let entries = self.overflow_entries();
        let nodes: Vec<MenuNode> = entries.iter().map(|(node, _)| node.clone()).collect();
        let targets: Vec<OnPress<State>> = entries.into_iter().map(|(_, target)| target).collect();

        frust::authoring::any::<State, _>(
            menu(nodes, move |state: &mut State, selection: MenuSelection| {
                if let Some(target) = targets.get(selection.index) {
                    target(state);
                }
            })
            .anchor(&anchor)
            .align(OverlayAlign::End)
            .open(open),
        )
    }

    /// The bar's resting elevation — the explicit override, else upstream's
    /// own `_hasFab ? elevationWithFab : elevation`.
    fn resolved_elevation(&self) -> ToolbarElevation {
        self.elevation.unwrap_or(if self.fab.is_some() {
            ToolbarElevation::Level1
        } else {
            ToolbarElevation::None
        })
    }

    /// The expand progress this configuration rests at: a bar with no morph to
    /// run is always fully expanded (upstream's own `startExpanded`).
    fn resting_progress(&self) -> f64 {
        if self.morphs() && !self.expanded {
            0.0
        } else {
            1.0
        }
    }

    /// Whether this bar runs the adjacent FAB morph at all — upstream's
    /// `_usesFabExpand` (`_hasFab && fabExpandsToolbar`, floating only).
    fn morphs(&self) -> bool {
        self.variant == ToolbarVariant::Floating && self.fab.is_some() && self.fab_expands_toolbar
    }

    /// Whether an adjacent FAB sits beside the pill rather than inside the row
    /// — upstream's `_hasFab` on a floating bar, morphing or not.
    fn adjacent_fab(&self) -> bool {
        self.variant == ToolbarVariant::Floating && self.fab.is_some()
    }
}

/// Collect `view`'s slot children into one ordered slice of [`AnyView`]
/// references — leading, center, the composed action views (held in
/// `actions`), trailing, then the fab slot. This is the shape both
/// `build`/`teardown` and [`frust::authoring::rebuild_children`] walk.
///
/// The composed action views cannot live in the view itself (they are derived
/// from the split, fresh per pass), so they are passed in alongside — the
/// one-slot storage handoff [`super::appbar`]'s own `title_ref` uses,
/// generalized to a list.
fn slot_views<'a, State: 'static>(
    view: &'a ToolbarView<State>,
    actions: &'a [AnyView<State>],
) -> Vec<&'a AnyView<State>> {
    let mut views = Vec::with_capacity(
        view.leading.len()
            + view.center.len()
            + actions.len()
            + view.trailing.len()
            + usize::from(view.fab.is_some()),
    );
    views.extend(view.leading.iter());
    views.extend(view.center.iter());
    views.extend(actions.iter());
    views.extend(view.trailing.iter());
    if let Some(fab) = &view.fab {
        views.push(fab);
    }
    views
}

// ---- The widget ------------------------------------------------------------

/// The retained widget for a [`ToolbarView`].
pub struct ToolbarWidget {
    variant: ToolbarVariant,
    color_style: ToolbarColorStyle,
    elevation: ToolbarElevation,
    /// Every slot pod, in the fixed order [`slot_views`] produces: leading,
    /// center, inline actions (plus the overflow trigger), trailing, then the
    /// fab slot — the one list [`frust::authoring::route_event`] hit-tests and
    /// [`frust::authoring::rebuild_children`] reconciles.
    slots: Vec<ChildPod>,
    leading_count: usize,
    center_count: usize,
    trailing_count: usize,
    has_fab: bool,
    adjacent_fab: bool,
    fab_expands: bool,
    fab_position: ToolbarFabPosition,
    semantic_label: Option<String>,
    exit_direction: ToolbarExitDirection,
    exit_extent: Option<f64>,
    /// The app-pinned hidden fraction, if any (bypasses [`Self::hide`]).
    hidden_pinned: Option<f64>,
    /// `0` collapsed through `1` expanded — the FAB morph and pill reveal.
    expand: RampTrack,
    /// `0` visible through `1` hidden — the scroll-hide travel.
    hide: RampTrack,
    /// The pill's own box, in bar-local px (set by `layout`, read by `paint`).
    pill_size: Size,
    /// The pill's origin within the bar's box, hide shift already applied.
    pill_origin: Point,
    /// The clamped expand progress `paint` clips and fades the pill with.
    reveal: f64,
}

impl<State: 'static> View<State> for ToolbarView<State> {
    type Element = ToolbarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToolbarWidget {
        let actions = self.action_views();
        let slots = slot_views(self, &actions)
            .into_iter()
            .map(|view| frust::authoring::build_child(view, ctx))
            .collect();
        let hidden = self
            .hidden_fraction
            .map_or(if self.visible { 0.0 } else { 1.0 }, clamp01);
        ToolbarWidget {
            variant: self.variant,
            color_style: self.color_style,
            elevation: self.resolved_elevation(),
            slots,
            leading_count: self.leading.len(),
            center_count: self.center.len() + actions.len(),
            trailing_count: self.trailing.len(),
            has_fab: self.fab.is_some(),
            adjacent_fab: self.adjacent_fab(),
            fab_expands: self.fab_expands_toolbar,
            fab_position: self.fab_position,
            semantic_label: self.semantic_label.clone(),
            exit_direction: self.exit_direction,
            exit_extent: self.exit_extent,
            hidden_pinned: self.hidden_fraction.map(clamp01),
            expand: RampTrack::resting(self.resting_progress()),
            hide: RampTrack::resting(hidden),
            pill_size: Size::ZERO,
            pill_origin: Point::ZERO,
            reveal: 1.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToolbarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        let prev_actions = prev.action_views();
        let next_actions = self.action_views();
        let prev_views = slot_views(prev, &prev_actions);
        let next_views = slot_views(self, &next_actions);
        flags |= frust::authoring::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.slots,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        let center_count = self.center.len() + next_actions.len();
        for (slot, next) in [
            (&mut element.leading_count, self.leading.len()),
            (&mut element.center_count, center_count),
            (&mut element.trailing_count, self.trailing.len()),
        ] {
            if *slot != next {
                *slot = next;
                flags |= ChangeFlags::LAYOUT;
            }
        }

        let now_fab = self.fab.is_some();
        if element.has_fab != now_fab {
            element.has_fab = now_fab;
            flags |= ChangeFlags::LAYOUT;
        }
        let adjacent = self.adjacent_fab();
        if element.adjacent_fab != adjacent {
            element.adjacent_fab = adjacent;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.fab_expands != self.fab_expands_toolbar {
            element.fab_expands = self.fab_expands_toolbar;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.fab_position != self.fab_position {
            element.fab_position = self.fab_position;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.color_style != self.color_style {
            element.color_style = self.color_style;
            flags |= ChangeFlags::PAINT;
        }
        let elevation = self.resolved_elevation();
        if element.elevation != elevation {
            element.elevation = elevation;
            flags |= ChangeFlags::PAINT;
        }
        if element.exit_direction != self.exit_direction {
            element.exit_direction = self.exit_direction;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.exit_extent != self.exit_extent {
            element.exit_extent = self.exit_extent;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.semantic_label != self.semantic_label {
            element.semantic_label = self.semantic_label.clone();
        }

        // Both ramps are layout values, so a retarget asks for a relayout
        // rather than only a repaint; `paint` keeps asking while the leg runs.
        let progress = self.resting_progress();
        if (element.expand.target - progress).abs() >= RAMP_EPSILON {
            element.expand.set_target(progress);
            flags |= ChangeFlags::LAYOUT;
        }

        let pinned = self.hidden_fraction.map(clamp01);
        element.hidden_pinned = pinned;
        match pinned {
            // A pinned fraction applies with no spring, and keeps the ramp in
            // step so an unpin resumes from where the drag left it.
            Some(fraction) => {
                if element.hide.value != fraction {
                    element.hide.jump(fraction);
                    flags |= ChangeFlags::LAYOUT;
                }
            }
            None => {
                let hidden = if self.visible { 0.0 } else { 1.0 };
                if (element.hide.target - hidden).abs() >= RAMP_EPSILON {
                    element.hide.set_target(hidden);
                    flags |= ChangeFlags::LAYOUT;
                }
            }
        }

        flags
    }

    fn teardown(&self, element: &mut ToolbarWidget, ctx: &mut BuildCtx<'_>) {
        let actions = self.action_views();
        for (view, pod) in slot_views(self, &actions)
            .into_iter()
            .zip(element.slots.iter_mut())
        {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl ToolbarWidget {
    /// Whether the adjacent FAB morphs with the pill (upstream's
    /// `_usesFabExpand`).
    fn morphs(&self) -> bool {
        self.adjacent_fab && self.fab_expands
    }

    /// The index of the fab pod, when there is one.
    fn fab_index(&self) -> Option<usize> {
        self.has_fab.then(|| self.slots.len() - 1)
    }

    /// How many pods lay out inside the pill's row: everything but an adjacent
    /// FAB, which takes the morph layout instead.
    fn row_len(&self) -> usize {
        let row = self.leading_count + self.center_count + self.trailing_count;
        if self.has_fab && !self.adjacent_fab {
            row + 1
        } else {
            row
        }
    }

    /// The hidden fraction this frame — the app's pin, else the ramp.
    fn hidden(&self) -> f64 {
        self.hidden_pinned
            .unwrap_or_else(|| clamp01(self.hide.value))
    }

    /// The revealed window of the bar's box, in bar-local px: the sub-rect the
    /// pill is clipped to while the reveal runs (upstream's own clip, further
    /// intersected with this widget's own box so nothing paints outside it).
    fn reveal_window(&self, box_size: Size) -> (Point, Size) {
        if !self.morphs() || self.reveal >= 1.0 {
            return (Point::ZERO, box_size);
        }
        let natural = self.pill_size.width;
        let (x0, x1) = match self.fab_position {
            // Reveals from the trailing edge: the pill's own leading edge
            // walks left as `progress` grows.
            ToolbarFabPosition::End => (
                self.pill_origin.x + natural * (1.0 - self.reveal),
                box_size.width,
            ),
            // Reveals from the leading edge instead; the pill itself stays put.
            ToolbarFabPosition::Start => (0.0, self.pill_origin.x + natural * self.reveal),
        };
        let x0 = x0.clamp(0.0, box_size.width);
        let x1 = x1.clamp(x0, box_size.width);
        (Point::new(x0, 0.0), Size::new(x1 - x0, box_size.height))
    }
}

impl Widget for ToolbarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let progress = if self.morphs() {
            self.expand.value
        } else {
            1.0
        };
        self.reveal = clamp01(progress);

        let row_len = self.row_len();
        let row_bc = BoxConstraints::loose(Size::new(f64::INFINITY, TOOLBAR_HEIGHT));
        let sizes: Vec<Size> = self.slots[..row_len]
            .iter_mut()
            .map(|pod| pod.layout_child(ctx, &row_bc))
            .collect();

        // The row, laid out in pill-local coordinates.
        let pill_width = match self.variant {
            ToolbarVariant::Floating => {
                // Self-sized: one contiguous row, a `TOOLBAR_GAP` between every
                // adjacent pair (naturally collapsing across an empty group's
                // boundary, since the gap is per-item, not per-group) and
                // `TOOLBAR_FLOATING_PAD` at both ends.
                let mut x = TOOLBAR_FLOATING_PAD;
                for (pod, size) in self.slots[..row_len].iter_mut().zip(sizes.iter()) {
                    pod.set_origin(Point::new(x, (TOOLBAR_HEIGHT - size.height) / 2.0));
                    x += size.width + TOOLBAR_GAP;
                }
                if row_len > 0 {
                    x -= TOOLBAR_GAP;
                }
                x + TOOLBAR_FLOATING_PAD
            }
            ToolbarVariant::Docked => {
                let width = finite_or_zero(bc.max().width);

                // Leading: left-anchored, sequential.
                let mut left = TOOLBAR_DOCKED_PAD_X;
                for (pod, size) in self.slots[..self.leading_count]
                    .iter_mut()
                    .zip(sizes[..self.leading_count].iter())
                {
                    pod.set_origin(Point::new(left, (TOOLBAR_HEIGHT - size.height) / 2.0));
                    left += size.width + TOOLBAR_GAP;
                }
                if self.leading_count > 0 {
                    left -= TOOLBAR_GAP;
                }

                // Trailing (+ the in-row fab, if any): right-anchored, reverse
                // order, so the tail lands with the last item flush against
                // the trailing edge and reading order preserved.
                let tail_start = self.leading_count + self.center_count;
                let mut right = width - TOOLBAR_DOCKED_PAD_X;
                for (pod, size) in self.slots[tail_start..row_len]
                    .iter_mut()
                    .zip(sizes[tail_start..row_len].iter())
                    .rev()
                {
                    right -= size.width;
                    pod.set_origin(Point::new(right, (TOOLBAR_HEIGHT - size.height) / 2.0));
                    right -= TOOLBAR_GAP;
                }
                if tail_start < row_len {
                    right += TOOLBAR_GAP;
                }

                // Center: centered as a group within `[left, right]`.
                let center_range = self.leading_count..tail_start;
                let mut center_total: f64 =
                    sizes[center_range.clone()].iter().map(|s| s.width).sum();
                if center_range.len() > 1 {
                    center_total += TOOLBAR_GAP * (center_range.len() - 1) as f64;
                }
                let available = (right - left).max(0.0);
                let mut cx = left + ((available - center_total) / 2.0).max(0.0);
                for (pod, size) in self.slots[center_range.clone()]
                    .iter_mut()
                    .zip(sizes[center_range].iter())
                {
                    pod.set_origin(Point::new(cx, (TOOLBAR_HEIGHT - size.height) / 2.0));
                    cx += size.width + TOOLBAR_GAP;
                }

                width
            }
        };
        self.pill_size = Size::new(pill_width, TOOLBAR_HEIGHT);

        // The bar's own box, and where the pill sits inside it — upstream's
        // `RenderM3EToolbarHorizontalFabLayout.performLayout` whenever a FAB
        // sits beside the pill.
        let (box_size, pill_origin) = if self.adjacent_fab {
            let total = Size::new(
                pill_width + TOOLBAR_TO_FAB_GAP + TOOLBAR_FAB_BASELINE,
                TOOLBAR_FAB_MEDIUM,
            );
            let revealed = pill_width * progress.clamp(0.0, REVEAL_OVERSHOOT);
            let x = match self.fab_position {
                ToolbarFabPosition::End => pill_width - revealed,
                ToolbarFabPosition::Start => total.width - pill_width,
            };
            (
                total,
                Point::new(x, (TOOLBAR_FAB_MEDIUM - TOOLBAR_HEIGHT) / 2.0),
            )
        } else {
            (self.pill_size, Point::ZERO)
        };

        // The adjacent FAB's own square. Tight constraints are what carry the
        // morph here — see the [module docs](self)' fab-seam note.
        if self.adjacent_fab
            && let Some(index) = self.fab_index()
        {
            let fab_size = if self.fab_expands {
                ToolbarMorph::size(progress)
            } else {
                TOOLBAR_FAB_BASELINE
            };
            let pod = &mut self.slots[index];
            pod.layout_child(ctx, &BoxConstraints::tight(Size::new(fab_size, fab_size)));
            let x = match self.fab_position {
                ToolbarFabPosition::End => box_size.width - fab_size,
                ToolbarFabPosition::Start => 0.0,
            };
            pod.set_origin(Point::new(x, (TOOLBAR_FAB_MEDIUM - fab_size) / 2.0));
        }

        // The scroll-hide travel: a layout value, applied to every pod (and to
        // the pill origin `paint` fills against) so hit-testing follows the
        // visual exactly.
        let hidden = self.hidden();
        // `finite_or_zero`-funneled: `exit_extent` is caller-fed, and a
        // non-finite value here would carry through `shift` into
        // `pill_origin` and, from there, into `reveal_window`'s clamp bound
        // — see the [module docs](self)' clamp discipline.
        let extent = finite_or_zero(self.exit_extent.unwrap_or(
            if self.exit_direction.is_vertical() {
                box_size.height + TOOLBAR_SCREEN_OFFSET
            } else {
                box_size.width + TOOLBAR_SCREEN_OFFSET
            },
        ));
        let shift = self.exit_direction.offset(hidden * extent.abs());
        self.pill_origin = pill_origin + shift;

        // Row pods sit in pill-local coordinates; lift them into the box, with
        // the travel already folded into the pill's own origin.
        let pill_shift = self.pill_origin.to_vec2();
        if pill_shift != Vec2::ZERO {
            for pod in self.slots[..row_len].iter_mut() {
                let origin = pod.origin();
                pod.set_origin(origin + pill_shift);
            }
        }
        // An adjacent FAB was placed in box coordinates already, so it takes
        // the travel alone.
        if self.adjacent_fab
            && shift != Vec2::ZERO
            && let Some(index) = self.fab_index()
        {
            let origin = self.slots[index].origin();
            self.slots[index].set_origin(origin + shift);
        }

        bc.constrain(box_size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Resolve every theme-derived value before the `&mut ctx` reborrows
        // below (`request_layout`) — `theme` borrows `ctx` shared, so its last
        // use must precede them for this to typecheck under NLL.
        let theme = Theme::from_paint_ctx(ctx);
        let colors = ToolbarColors::resolve(theme, self.color_style);
        let shadow = resolve_shadow(theme, self.elevation);
        let radius = resolve_radius(
            theme,
            self.variant,
            self.pill_size.width,
            self.pill_size.height,
        );
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();

        // Both ramps drive layout values (the FAB square, the hide travel), so
        // an in-flight leg needs an explicit relayout, not merely another
        // frame — the layout-skip class the module docs describe. Either arm
        // asks: still in flight (the next frame moves it again), or the value
        // moved this frame — the *settling* frame is the one that both stops
        // animating and lands on the target, so gating on the animating flag
        // alone would leave the bar a fraction short of rest.
        let moved = if reduce_motion {
            let expand = self.expand.snap();
            let hide = self.hide.snap();
            expand || hide
        } else {
            let before = (self.expand.value, self.hide.value);
            let expand = self.expand.advance(now);
            let hide = self.hide.advance(now);
            expand || hide || (self.expand.value, self.hide.value) != before
        };
        if moved {
            ctx.request_layout();
        }

        let origin = ctx.origin();
        let box_size = ctx.size();
        let alpha = if self.morphs() {
            ToolbarMorph::reveal_alpha(self.reveal) as f32
        } else {
            1.0
        };
        let (window_origin, window_size) = self.reveal_window(box_size);
        let clipping =
            window_origin != Point::ZERO || window_size != box_size || self.hidden() > 0.0;

        if clipping {
            scene.push_clip(
                Point::new(origin.x + window_origin.x, origin.y + window_origin.y),
                window_size,
            );
        }
        if alpha < 1.0 {
            scene.push_layer(origin, box_size, alpha);
        }

        let pill_origin = Point::new(origin.x + self.pill_origin.x, origin.y + self.pill_origin.y);
        if let Some((blur, y_offset, shadow_color)) = shadow {
            scene.draw_shadow(
                Point::new(pill_origin.x, pill_origin.y + y_offset),
                self.pill_size,
                radius,
                blur,
                shadow_color,
            );
        }
        scene.fill_rounded_rect(pill_origin, self.pill_size, radius, colors.container);

        let row_len = self.row_len();
        for pod in self.slots[..row_len].iter_mut() {
            pod.paint_child(ctx, scene);
        }

        if alpha < 1.0 {
            scene.pop_layer();
        }
        if clipping {
            scene.pop_clip();
        }

        // An adjacent FAB is outside the pill's reveal clip and fade entirely
        // (upstream paints it after, unclipped) — it is the one thing on
        // screen while the pill is fully collapsed.
        if self.adjacent_fab
            && let Some(index) = self.fab_index()
        {
            self.slots[index].paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.slots, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let slots = &self.slots;
        let label = self.semantic_label.as_deref();
        ctx.push_container(
            Role::Toolbar,
            |node| {
                if let Some(label) = label {
                    node.set_label(label);
                }
            },
            |ctx| {
                for pod in slots {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(slots);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::BuildCtx;
    use frust_widgets::test_support::leaf_any;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    /// Records each `fill_rounded_rect` call's `(origin, size, radius,
    /// color)` — this widget paints its container as a rounded rect, even at
    /// a docked-variant radius of `0.0`, so the shared
    /// [`frust_widgets::test_support::RecordingScene`] (which only records
    /// `fill_rect`) can't observe it.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size)>,
        clips: usize,
        layers: Vec<f32>,
    }
    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, _r: f64, _sd: f64, _c: Color) {
            self.shadows.push((o, s));
        }
        fn push_clip(&mut self, _o: Point, _s: Size) {
            self.clips += 1;
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    fn build(view: &ToolbarView<()>) -> ToolbarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    fn layout(w: &mut ToolbarWidget, bc: &BoxConstraints) -> Size {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, bc)
    }

    /// Paint one frame `millis` into the ramp, reporting whether a relayout
    /// was requested (the in-flight/settling signal).
    fn paint_at(w: &mut ToolbarWidget, millis: u64, theme: Option<&Theme>) -> bool {
        let mut scene = RRectRecorder::default();
        let mut pctx = PaintCtx::for_test(
            Point::ZERO,
            Size::new(400.0, TOOLBAR_FAB_MEDIUM),
            FrameTime::from_nanos(millis * 1_000_000),
        );
        if let Some(theme) = theme {
            pctx = pctx.with_theme(theme);
        }
        w.paint(&mut pctx, &mut scene);
        pctx.needs_layout()
    }

    // ---- Metrics ----------------------------------------------------------

    #[test]
    fn the_ported_tokens_match_the_reference_table_exactly() {
        assert_eq!(TOOLBAR_HEIGHT, 64.0);
        assert_eq!(TOOLBAR_FLOATING_PAD, 8.0);
        assert_eq!(TOOLBAR_DOCKED_PAD_X, 16.0);
        assert_eq!(TOOLBAR_GAP, 4.0);
        assert_eq!(TOOLBAR_TO_FAB_GAP, 8.0);
        assert_eq!(TOOLBAR_SCREEN_OFFSET, 16.0);
        assert_eq!(TOOLBAR_FAB_BASELINE, 56.0);
        assert_eq!(TOOLBAR_FAB_MEDIUM, 80.0);
        assert_eq!(TOOLBAR_MAX_INLINE_ACTIONS, 4);
        assert_eq!(TOOLBAR_SETTLE_VELOCITY, 150.0);
    }

    #[test]
    fn the_expand_spring_is_the_reference_expressive_spatial_fast_token() {
        let token = crate::MaterialSpring::EXPRESSIVE_SPATIAL_FAST;
        assert_eq!(TOOLBAR_EXPAND_SPRING.stiffness, token.stiffness);
        assert_eq!(TOOLBAR_EXPAND_SPRING.damping_ratio, token.damping_ratio);
        assert_eq!(TOOLBAR_EXPAND_SPRING.mass, 1.0);
    }

    // ---- Layout: the two variants -----------------------------------------

    #[test]
    fn floating_layout_preserves_slot_order_and_applies_inset_pill_geometry() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .center(vec![leaf_any(20.0, 20.0)])
            .trailing(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        // A floating bar with a FAB reserves the adjacent morph box.
        assert_eq!(size.height, TOOLBAR_FAB_MEDIUM);
        assert!(size.width < 800.0);

        // Slot order preserved: leading < center < trailing, left to right.
        assert!(w.slots[0].origin().x < w.slots[1].origin().x);
        assert!(w.slots[1].origin().x < w.slots[2].origin().x);

        // Inset from the pill's leading edge by `TOOLBAR_FLOATING_PAD`.
        assert_eq!(w.slots[0].origin().x, TOOLBAR_FLOATING_PAD);

        // A fully-rounded pill: radius resolves to half the fixed bar height
        // (the shorter side), the `ShapeScale::full` clamp.
        let radius = resolve_radius(
            None,
            ToolbarVariant::Floating,
            w.pill_size.width,
            w.pill_size.height,
        );
        assert_eq!(radius, TOOLBAR_HEIGHT / 2.0);
    }

    #[test]
    fn a_floating_toolbar_without_a_fab_hugs_its_own_content() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .trailing(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        assert_eq!(size.height, TOOLBAR_HEIGHT);
        assert_eq!(
            size.width,
            TOOLBAR_FLOATING_PAD * 2.0 + 24.0 + TOOLBAR_GAP + 24.0,
            "pad + item + gap + item + pad"
        );
        assert_eq!(
            w.slots[1].origin().x + w.slots[1].size().width,
            size.width - TOOLBAR_FLOATING_PAD
        );
    }

    #[test]
    fn docked_layout_spans_the_available_width_with_flat_geometry() {
        let view: ToolbarView<()> = docked_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .trailing(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(size, Size::new(400.0, TOOLBAR_HEIGHT));

        let radius = resolve_radius(None, ToolbarVariant::Docked, size.width, size.height);
        assert_eq!(radius, 0.0, "docked variant is flat (square corners)");

        assert_eq!(w.slots[0].origin().x, TOOLBAR_DOCKED_PAD_X);
        assert_eq!(
            w.slots[1].origin().x + w.slots[1].size().width,
            400.0 - TOOLBAR_DOCKED_PAD_X
        );
    }

    #[test]
    fn docked_layout_centers_the_center_group_between_leading_and_trailing() {
        let view: ToolbarView<()> = docked_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .center(vec![leaf_any(20.0, 20.0)])
            .trailing(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        assert!(w.slots[0].origin().x < w.slots[1].origin().x);
        assert!(w.slots[1].origin().x < w.slots[2].origin().x);

        let leading_end = w.slots[0].origin().x + w.slots[0].size().width;
        let trailing_start = w.slots[2].origin().x;
        let center_start = w.slots[1].origin().x;
        let center_end = center_start + w.slots[1].size().width;
        let left_gap = center_start - leading_end;
        let right_gap = trailing_start - center_end;
        assert!(
            (left_gap - right_gap).abs() < 0.01,
            "the center group is centered in the remaining space \
             (left_gap={left_gap}, right_gap={right_gap})"
        );
    }

    #[test]
    fn docked_layout_places_fab_flush_against_the_trailing_edge_after_trailing() {
        let view: ToolbarView<()> = docked_toolbar()
            .trailing(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        // Docked keeps the fab in-row: the box is still one bar tall.
        assert_eq!(size, Size::new(400.0, TOOLBAR_HEIGHT));
        assert!(w.slots[0].origin().x < w.slots[1].origin().x);
        assert_eq!(
            w.slots[1].origin().x + w.slots[1].size().width,
            400.0 - TOOLBAR_DOCKED_PAD_X,
            "the fab slot sits flush against the trailing edge"
        );
    }

    // ---- The FAB morph ----------------------------------------------------

    #[test]
    fn the_fab_morphs_from_80_collapsed_to_56_expanded() {
        assert_eq!(ToolbarMorph::size(0.0), TOOLBAR_FAB_MEDIUM);
        assert_eq!(ToolbarMorph::size(1.0), TOOLBAR_FAB_BASELINE);
        assert_eq!(ToolbarMorph::size(0.5), 68.0);
        // The expand spring's overshoot is real travel, never clamped away…
        assert!(ToolbarMorph::size(1.2) < TOOLBAR_FAB_BASELINE);
        // …but the square never goes negative, and NaN rests collapsed.
        assert_eq!(ToolbarMorph::size(100.0), 0.0);
        assert_eq!(ToolbarMorph::size(f64::NAN), TOOLBAR_FAB_MEDIUM);
    }

    #[test]
    fn a_collapsed_bar_lays_its_fab_out_at_the_80dp_end_and_hides_the_pill() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0))
            .expanded(false);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        let fab = w.slots[w.fab_index().expect("a fab pod")].size();
        assert_eq!(fab, Size::new(TOOLBAR_FAB_MEDIUM, TOOLBAR_FAB_MEDIUM));
        assert_eq!(size.height, TOOLBAR_FAB_MEDIUM);
        assert_eq!(w.reveal, 0.0, "collapsed: nothing of the pill is revealed");

        // The reveal window opens past the pill's own trailing edge, so none
        // of the pill paints at all — only the FAB is on screen.
        let (window_origin, _) = w.reveal_window(size);
        assert!(window_origin.x >= w.pill_origin.x + w.pill_size.width);
    }

    /// A NaN `exit_extent` prop must not corrupt `pill_origin` into a value
    /// `reveal_window`'s clamp would abort on — `x0` becomes the second
    /// clamp's `min` bound there, so an unguarded NaN reaching it panics on
    /// `assert!(min <= max)`, not merely on the clamped value itself. This is
    /// the layout/paint counterpart to the constructor guard above; see the
    /// module docs' clamp discipline.
    #[test]
    fn a_non_finite_exit_extent_prop_reaches_layout_and_paint_without_panicking() {
        for extent in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let view: ToolbarView<()> = floating_toolbar()
                .leading(vec![leaf_any(24.0, 24.0)])
                .fab(leaf_any(40.0, 40.0))
                .expanded(false)
                .exit_extent(Some(extent));
            let mut w = build(&view);
            let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

            assert!(w.pill_origin.x.is_finite());
            assert!(w.pill_origin.y.is_finite());

            let (window_origin, window_size) = w.reveal_window(size);
            assert!(window_origin.x.is_finite());
            assert!(window_size.width.is_finite());

            // Painting must not panic either — the same clamp reruns inside
            // `paint`.
            paint_at(&mut w, 0, None);
        }
    }

    #[test]
    fn an_expanded_bar_lays_its_fab_out_at_the_56dp_end_beside_the_pill() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        let index = w.fab_index().expect("a fab pod");
        assert_eq!(
            w.slots[index].size(),
            Size::new(TOOLBAR_FAB_BASELINE, TOOLBAR_FAB_BASELINE)
        );
        // The box always reserves `fabBaseline` beside the pill, whatever the
        // morph is currently painting.
        assert_eq!(
            size.width,
            w.pill_size.width + TOOLBAR_TO_FAB_GAP + TOOLBAR_FAB_BASELINE
        );
        // Expanded, the pill sits flush at the box's leading edge and the FAB
        // flush at its trailing one.
        assert_eq!(w.pill_origin.x, 0.0);
        assert_eq!(
            w.slots[index].origin().x + TOOLBAR_FAB_BASELINE,
            size.width,
            "the fab hugs the trailing edge"
        );
        assert_eq!(w.reveal, 1.0);
    }

    #[test]
    fn pinning_fab_expands_toolbar_off_keeps_the_fab_at_the_baseline_square() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0))
            .fab_expands_toolbar(false)
            .expanded(false);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        let index = w.fab_index().expect("a fab pod");
        assert_eq!(
            w.slots[index].size(),
            Size::new(TOOLBAR_FAB_BASELINE, TOOLBAR_FAB_BASELINE)
        );
        assert_eq!(w.reveal, 1.0, "the pill stays open");
    }

    #[test]
    fn a_start_positioned_fab_leads_the_pill() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0))
            .fab_position(ToolbarFabPosition::Start);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        let index = w.fab_index().expect("a fab pod");
        assert_eq!(w.slots[index].origin().x, 0.0);
        assert_eq!(w.pill_origin.x, size.width - w.pill_size.width);
    }

    #[test]
    fn the_expand_ramp_settles_onto_the_target_and_stops_requesting_layout() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0))
            .expanded(false);
        let next: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));

        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));
        assert!(!paint_at(&mut w, 0, None), "a resting bar asks for nothing");

        let flags = View::<()>::rebuild(&next, &view, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout(), "a retarget is a layout change");

        let mut frames = 0u64;
        loop {
            layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));
            let still = paint_at(&mut w, frames * 16, None);
            frames += 1;
            if !still {
                break;
            }
            assert!(frames < 400, "the ramp settles well inside 400 frames");
        }
        layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));
        assert_eq!(w.expand.value, 1.0, "the settled ramp lands on the target");
        assert_eq!(
            w.slots[w.fab_index().expect("a fab pod")].size().width,
            TOOLBAR_FAB_BASELINE
        );
    }

    #[test]
    fn reduce_motion_snaps_both_ramps_and_requests_exactly_one_relayout() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;

        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0))
            .expanded(false);
        let next: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));

        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));
        View::<()>::rebuild(&next, &view, &mut w, &mut ctx(&mut counter));

        assert!(
            paint_at(&mut w, 16, Some(&theme)),
            "the snap moved a layout value, so it asks for one relayout"
        );
        assert_eq!(w.expand.value, 1.0, "snapped");
        assert!(
            !paint_at(&mut w, 32, Some(&theme)),
            "and never asks again once there is nothing left to move"
        );
    }

    // ---- Scroll-hide ------------------------------------------------------

    #[test]
    fn the_measured_exit_limit_is_the_bar_extent_plus_the_screen_offset() {
        let hide = ToolbarScrollHide::for_bar(TOOLBAR_HEIGHT);
        assert_eq!(
            hide.offset_limit(),
            -(TOOLBAR_HEIGHT + TOOLBAR_SCREEN_OFFSET)
        );
        assert_eq!(hide.collapsed_fraction(), 0.0);
        assert!(!hide.is_hidden());

        // An explicit extent pins the limit instead, sign-independently.
        assert_eq!(ToolbarScrollHide::new(-40.0).offset_limit(), -40.0);
    }

    /// A degenerate (NaN/∞) construction parameter must degrade to a resting
    /// controller, never panic — the module docs' clamp-discipline
    /// funnel-at-entry rule, exercised through both constructors and a
    /// following `scroll()` sequence (the site that would otherwise abort on
    /// a non-finite `limit`).
    #[test]
    fn a_non_finite_construction_parameter_degrades_instead_of_panicking() {
        for degenerate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut hide = ToolbarScrollHide::new(degenerate);
            assert_eq!(hide.offset_limit(), 0.0, "degrades to a resting limit");
            // A subsequent scroll/settle/toggle sequence must not panic —
            // the site that would otherwise abort on a non-finite `limit` —
            // and every reachable state stays finite.
            hide.scroll(40.0);
            hide.scroll(-1_000.0);
            hide.settle(1_000.0);
            hide.toggle();
            assert!(hide.offset().is_finite());

            let mut hide = ToolbarScrollHide::for_bar(degenerate);
            assert_eq!(
                hide.offset_limit(),
                0.0,
                "for_bar degrades the same way, delegating to new"
            );
            hide.scroll(degenerate);
            hide.scroll(40.0);
            hide.toggle();
            assert!(hide.offset().is_finite());
        }
    }

    #[test]
    fn scrolling_accumulates_into_a_clamped_hidden_fraction() {
        let mut hide = ToolbarScrollHide::new(80.0);
        hide.scroll(40.0);
        assert_eq!(hide.offset(), -40.0);
        assert_eq!(hide.collapsed_fraction(), 0.5);

        // Scrolling back up returns the bar.
        hide.scroll(-15.0);
        assert_eq!(hide.offset(), -25.0);

        // Both ends clamp, and a non-finite delta is ignored outright.
        hide.scroll(1_000.0);
        assert_eq!(hide.offset(), -80.0);
        assert!(hide.is_hidden());
        hide.scroll(f64::NAN);
        assert_eq!(hide.offset(), -80.0);
        hide.scroll(-1_000.0);
        assert_eq!(hide.offset(), 0.0);
    }

    #[test]
    fn a_settle_commits_to_the_fling_direction_past_the_velocity_threshold() {
        let mut hide = ToolbarScrollHide::new(80.0);
        hide.scroll(10.0);
        // A downward fling hides even from barely-moved.
        assert!(!hide.settle(TOOLBAR_SETTLE_VELOCITY + 1.0));
        assert!(hide.is_hidden());

        let mut hide = ToolbarScrollHide::new(80.0);
        hide.scroll(70.0);
        // An upward fling shows even from nearly-hidden.
        assert!(hide.settle(-(TOOLBAR_SETTLE_VELOCITY + 1.0)));
        assert_eq!(hide.offset(), 0.0);

        // Under the threshold the nearer end wins.
        let mut hide = ToolbarScrollHide::new(80.0);
        hide.scroll(30.0);
        assert!(hide.settle(10.0));
        let mut hide = ToolbarScrollHide::new(80.0);
        hide.scroll(50.0);
        assert!(!hide.settle(10.0));

        // Already settled, nothing moves.
        let mut hide = ToolbarScrollHide::new(80.0);
        assert!(hide.settle(1_000.0));
        assert_eq!(hide.offset(), 0.0);
    }

    #[test]
    fn toggle_flips_about_the_halfway_point() {
        let mut hide = ToolbarScrollHide::new(80.0);
        hide.toggle();
        assert!(hide.is_hidden());
        hide.toggle();
        assert_eq!(hide.offset(), 0.0);
    }

    #[test]
    fn a_pinned_hidden_fraction_translates_every_slot_along_the_exit_direction() {
        let visible: ToolbarView<()> = docked_toolbar().leading(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&visible);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        let resting = w.slots[0].origin();

        let hidden: ToolbarView<()> = docked_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .hidden_fraction(Some(1.0));
        let mut counter = 0u64;
        let flags = View::<()>::rebuild(&hidden, &visible, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout(), "the hide travel is a layout value");
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        // Bottom exit: the whole bar slides down by its own extent plus the
        // screen offset, so nothing of it remains inside the box.
        let travel = size.height + TOOLBAR_SCREEN_OFFSET;
        assert_eq!(w.slots[0].origin().y, resting.y + travel);
        assert_eq!(w.pill_origin.y, travel);
        assert!(w.slots[0].origin().y >= size.height);
    }

    #[test]
    fn every_exit_direction_translates_on_its_own_axis() {
        for (direction, expected) in [
            (ToolbarExitDirection::Top, Vec2::new(0.0, -10.0)),
            (ToolbarExitDirection::Bottom, Vec2::new(0.0, 10.0)),
            (ToolbarExitDirection::Start, Vec2::new(-10.0, 0.0)),
            (ToolbarExitDirection::End, Vec2::new(10.0, 0.0)),
        ] {
            assert_eq!(direction.offset(10.0), expected);
        }
        assert!(ToolbarExitDirection::Top.is_vertical());
        assert!(!ToolbarExitDirection::End.is_vertical());
    }

    #[test]
    fn an_explicit_exit_extent_overrides_the_measured_one() {
        let view: ToolbarView<()> = docked_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .hidden_fraction(Some(1.0))
            .exit_extent(Some(20.0));
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(w.pill_origin.y, 20.0);
    }

    #[test]
    fn the_visible_prop_springs_the_hide_travel_and_settles_hidden() {
        let visible: ToolbarView<()> = docked_toolbar().leading(vec![leaf_any(24.0, 24.0)]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&visible, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        let hidden: ToolbarView<()> = docked_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .visible(false);
        View::<()>::rebuild(&hidden, &visible, &mut w, &mut ctx(&mut counter));

        let mut frames = 0u64;
        loop {
            layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
            let still = paint_at(&mut w, frames * 16, None);
            frames += 1;
            if !still {
                break;
            }
            assert!(frames < 400, "the hide ramp settles well inside 400 frames");
        }
        assert_eq!(w.hidden(), 1.0, "the settled ramp lands fully hidden");
    }

    // ---- Overflow ---------------------------------------------------------

    #[test]
    fn the_capacity_rule_matches_the_reference_partition() {
        // Everything fits: nothing overflows and there is no trigger slot.
        let split = partition_actions(3, TOOLBAR_MAX_INLINE_ACTIONS, None);
        assert_eq!(split.inline, vec![0, 1, 2]);
        assert!(split.overflow.is_empty());

        // Surplus actions overflow in order.
        let split = partition_actions(7, TOOLBAR_MAX_INLINE_ACTIONS, None);
        assert_eq!(split.inline, vec![0, 1, 2, 3]);
        assert_eq!(split.overflow, vec![4, 5, 6]);

        // An expand trigger always stays inline and reserves one slot.
        let split = partition_actions(7, TOOLBAR_MAX_INLINE_ACTIONS, Some(0));
        assert_eq!(split.inline, vec![0, 1, 2, 3]);
        assert_eq!(split.overflow, vec![4, 5, 6]);
        let split = partition_actions(7, TOOLBAR_MAX_INLINE_ACTIONS, Some(6));
        assert_eq!(split.inline, vec![0, 1, 2, 6]);
        assert_eq!(split.overflow, vec![3, 4, 5]);

        // A zero budget overflows everything but the trigger.
        let split = partition_actions(3, 0, Some(1));
        assert_eq!(split.inline, vec![1]);
        assert_eq!(split.overflow, vec![0, 2]);

        // An out-of-range trigger index is ignored rather than reserving.
        let split = partition_actions(2, 1, Some(9));
        assert_eq!(split.inline, vec![0]);
        assert_eq!(split.overflow, vec![1]);
    }

    fn action(label: &str) -> ToolbarAction<Vec<String>> {
        let owned = label.to_owned();
        toolbar_action::<Vec<String>, _>(crate::icons::MORE_HORIZ, move |log: &mut Vec<String>| {
            log.push(owned.clone())
        })
        .label(label)
    }

    #[test]
    fn overflowed_actions_become_menu_rows_routing_back_to_their_own_action() {
        let view: ToolbarView<Vec<String>> = floating_toolbar()
            .actions(vec![
                action("one"),
                action("two"),
                action("three"),
                action("four"),
                action("five"),
                action("six"),
            ])
            .max_inline_actions(4);

        let entries = view.overflow_entries();
        assert_eq!(entries.len(), 2, "two actions past the budget");
        let labels: Vec<String> = entries
            .iter()
            .map(|(node, _)| match node {
                MenuNode::Entry(entry) => entry.label().to_owned(),
                _ => unreachable!("an overflowed action is always a plain entry"),
            })
            .collect();
        assert_eq!(labels, vec!["five".to_owned(), "six".to_owned()]);

        // Activating row `k` runs the action it was split from.
        let mut log: Vec<String> = Vec::new();
        entries[1].1(&mut log);
        assert_eq!(log, vec!["six".to_owned()]);
    }

    #[test]
    fn an_overflowed_row_falls_back_through_the_reference_label_chain() {
        let bare = toolbar_action::<(), _>(crate::icons::MORE_VERT, |_| {});
        assert_eq!(bare.menu_label(3), "Action 4");
        assert_eq!(
            toolbar_action::<(), _>(crate::icons::MORE_VERT, |_| {})
                .tooltip("Tip")
                .menu_label(0),
            "Tip"
        );
        assert_eq!(
            toolbar_action::<(), _>(crate::icons::MORE_VERT, |_| {})
                .tooltip("Tip")
                .label("Label")
                .menu_label(0),
            "Label"
        );
        assert_eq!(
            toolbar_action::<(), _>(crate::icons::MORE_VERT, |_| {})
                .semantic_label("Named")
                .menu_label(0),
            "Named"
        );
    }

    #[test]
    fn the_overflow_trigger_joins_the_row_only_once_something_overflows() {
        let fits: ToolbarView<Vec<String>> =
            floating_toolbar().actions(vec![action("one"), action("two")]);
        let mut w = build_any(&fits);
        assert_eq!(w.center_count, 2, "no trigger while everything fits");

        let spills: ToolbarView<Vec<String>> = floating_toolbar().actions(vec![
            action("one"),
            action("two"),
            action("three"),
            action("four"),
            action("five"),
        ]);
        w = build_any(&spills);
        assert_eq!(w.center_count, 5, "four inline actions plus the trigger");
        assert_eq!(w.slots.len(), 5);
    }

    fn build_any<State: 'static>(view: &ToolbarView<State>) -> ToolbarWidget {
        let mut counter = 0u64;
        View::<State>::build(view, &mut ctx(&mut counter))
    }

    // ---- Paint -------------------------------------------------------------

    #[test]
    fn unthemed_paint_uses_the_standard_fallback_container() {
        let view: ToolbarView<()> = docked_toolbar();
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = RRectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, TOOLBAR_HEIGHT));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rrects[0].1, Size::new(300.0, TOOLBAR_HEIGHT));
        assert_eq!(scene.rrects[0].3, SURFACE_CONTAINER);
        assert!(
            scene.shadows.is_empty(),
            "a bar with no fab rests at `elevationNone`"
        );
    }

    #[test]
    fn themed_paint_resolves_surface_container() {
        let theme = crate::baseline();
        let view: ToolbarView<()> = floating_toolbar().leading(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));

        let mut scene = RRectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rrects[0].3, theme.scheme().surface_container);
        assert_eq!(
            scene.rrects[0].2,
            ShapeScale::resolve(theme.shape.full, size.width, size.height)
        );
    }

    #[test]
    fn the_vibrant_style_resolves_the_reference_primary_container_pair() {
        let theme = crate::baseline();
        let standard = ToolbarColors::resolve(Some(&theme), ToolbarColorStyle::Standard);
        assert_eq!(standard.container, theme.scheme().surface_container);
        assert_eq!(standard.content, theme.scheme().on_surface);
        assert_eq!(standard.fab_container, theme.scheme().primary_container);
        assert_eq!(standard.fab_content, theme.scheme().on_primary_container);

        let vibrant = ToolbarColors::resolve(Some(&theme), ToolbarColorStyle::Vibrant);
        assert_eq!(vibrant.container, theme.scheme().primary_container);
        assert_eq!(vibrant.content, theme.scheme().on_primary_container);
        assert_eq!(vibrant.fab_container, theme.scheme().tertiary_container);
        assert_eq!(vibrant.fab_content, theme.scheme().on_tertiary_container);

        // Unthemed, the same four roles resolve to their baseline literals.
        let unthemed = ToolbarColors::resolve(None, ToolbarColorStyle::Vibrant);
        assert_eq!(unthemed.container, PRIMARY_CONTAINER);
        assert_eq!(unthemed.fab_container, TERTIARY_CONTAINER);

        let view: ToolbarView<()> = docked_toolbar().color_style(ToolbarColorStyle::Vibrant);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = RRectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rrects[0].3, theme.scheme().primary_container);
    }

    #[test]
    fn the_elevation_pair_follows_the_reference_rule_with_an_explicit_override() {
        // No fab: `elevationNone`, i.e. no shadow at all.
        let plain: ToolbarView<()> = floating_toolbar();
        assert_eq!(plain.resolved_elevation(), ToolbarElevation::None);
        assert!(resolve_shadow(None, ToolbarElevation::None).is_none());

        // With a fab: `elevationWithFabExpanded`, i.e. M3 level 1.
        let with_fab: ToolbarView<()> = floating_toolbar().fab(leaf_any(40.0, 40.0));
        assert_eq!(with_fab.resolved_elevation(), ToolbarElevation::Level1);
        let (blur, y, _) =
            resolve_shadow(None, ToolbarElevation::Level1).expect("a level-1 shadow");
        assert_eq!((blur, y), (LEVEL1_SHADOW_BLUR, LEVEL1_SHADOW_Y_OFFSET));

        // The explicit override wins over both.
        let pinned: ToolbarView<()> = floating_toolbar()
            .fab(leaf_any(40.0, 40.0))
            .elevation(ToolbarElevation::Level2);
        assert_eq!(pinned.resolved_elevation(), ToolbarElevation::Level2);
        let (blur, y, _) =
            resolve_shadow(None, ToolbarElevation::Level2).expect("a level-2 shadow");
        assert_eq!((blur, y), (LEVEL2_SHADOW_BLUR, LEVEL2_SHADOW_Y_OFFSET));

        let theme = crate::baseline();
        let (blur, y, _) =
            resolve_shadow(Some(&theme), ToolbarElevation::Level1).expect("a themed shadow");
        let spec = theme.elevation.level1.shadow(theme.brightness);
        assert_eq!((blur, y), (spec.blur_std_dev, spec.y_offset));
    }

    #[test]
    fn a_revealing_pill_is_clipped_and_faded_while_the_fab_stays_opaque() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0))
            .expanded(false);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        let mut scene = RRectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(200.0, TOOLBAR_FAB_MEDIUM));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.clips, 1, "the pill paints inside a reveal clip");
        assert_eq!(
            scene.layers,
            vec![0.0],
            "collapsed, the pill is fully faded out"
        );

        // The fade only starts halfway through the ramp — upstream's own
        // `Interval(0.5, 1, easeIn)`.
        assert_eq!(ToolbarMorph::reveal_alpha(0.0), 0.0);
        assert_eq!(ToolbarMorph::reveal_alpha(0.5), 0.0);
        assert_eq!(ToolbarMorph::reveal_alpha(1.0), 1.0);
        assert!(ToolbarMorph::reveal_alpha(0.75) > 0.0);
    }

    // ---- Semantics + rebuild ----------------------------------------------

    #[test]
    fn semantics_node_is_toolbar_forwarding_every_slot() {
        // A leaf without its own `semantics()` override (e.g. `leaf_any`)
        // contributes no node — use `Text` children instead, so the
        // "forwards every slot" assertion is meaningful.
        fn logic(_state: &mut ()) -> ToolbarView<()> {
            docked_toolbar()
                .semantic_label("Editing")
                .leading(vec![frust::authoring::any::<(), _>(frust::text("Leading"))])
                .trailing(vec![frust::authoring::any::<(), _>(frust::text(
                    "Trailing",
                ))])
        }
        let mut root: frust_core::RenderRoot<(), ToolbarView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(
            Size::new(300.0, TOOLBAR_HEIGHT),
            &mut tcx as &mut dyn std::any::Any,
        );
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Toolbar)
            .expect("a Toolbar node is contributed");
        assert_eq!(node.children().len(), 2, "leading + trailing forwarded");
        assert_eq!(node.label(), Some("Editing"));
    }

    #[test]
    fn rebuild_adopts_new_slot_counts() {
        let mut counter = 0u64;
        let prev: ToolbarView<()> = floating_toolbar().leading(vec![leaf_any(24.0, 24.0)]);
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        assert_eq!(w.leading_count, 1);

        let next: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .trailing(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]);
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.trailing_count, 2);
        assert_eq!(w.slots.len(), 3);
    }

    #[test]
    fn attaching_a_fab_switches_a_floating_bar_onto_the_adjacent_layout() {
        let mut counter = 0u64;
        let prev: ToolbarView<()> = floating_toolbar().leading(vec![leaf_any(24.0, 24.0)]);
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));
        assert_eq!(size.height, TOOLBAR_HEIGHT);

        let next: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));
        let flags = View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        assert!(flags.needs_layout());
        assert!(w.adjacent_fab);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));
        assert_eq!(size.height, TOOLBAR_FAB_MEDIUM);
    }
}
