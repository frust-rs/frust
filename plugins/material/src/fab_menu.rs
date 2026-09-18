//! The Material 3 Expressive **FAB menu**: a trigger FAB that reveals a
//! vertical stack of large menu items on tap, replacing the older speed-dial
//! pattern (m3.material.io/blog/building-with-m3-expressive; Compose
//! `FloatingActionButtonMenu`).
//!
//! [`fab_menu`] takes the trigger's icon, the controlled `open` flag, and a
//! list of [`FabMenuItem`]s (built with [`fab_menu_item`]: icon + label +
//! `on_select`). It is a **controlled component** (see `docs/CODE_STANDARDS.md`'s
//! Interaction Semantics), mirroring [`mod@super::split_button`]'s `open` prop: the
//! widget never flips `open` itself. A single `on_toggle` callback fires on
//! every gesture that requests an open/close flip — a tap on the closed
//! trigger, a tap on the open trigger ("tap-again"), a completed tap on a menu
//! item (after that item's own `on_select` runs), and a tap on the scrim
//! (anywhere else in the widget's area while open) — and the app is expected
//! to toggle its own `open` state in that handler, exactly like
//! [`mod@super::split_button`]'s `on_open` convention. Passing a different
//! `icon` view on that rebuild (e.g. a close glyph while `open`) is how an app
//! reaches upstream's `M3EFabMenu.closeIcon`/`expandIcon`/`collapseIcon`
//! quartet — this module takes one `icon` slot and lets the caller's own
//! `open`-conditional branch decide its content, rather than adding four
//! icon props of its own.
//!
//! # Attribution
//!
//! Ported from `paadevelopments/material_3_expressive` v1.0.8 (MIT, © 2026 Paa
//! Developments), `lib/components/fab_menu/` (`m3e_fab_menu.dart`,
//! `enums/m3e_fab_menu_position.dart`, `models/m3e_fab_menu_item.dart`,
//! `styles/m3e_fab_menu_theme.dart`). See `plugins/material/NOTICE`'s Module
//! Attribution Header Convention.
//!
//! # Reveal geometry
//!
//! The trigger sits `EDGE_MARGIN` from the widget's own bottom-right corner
//! (this widget fills its box constraints, like [`super::sheet`], so it is
//! meant to be the top layer of a full-area [`frust::Stack`]) — a v1
//! divergence from upstream, whose `M3EFabMenu` self-sizes and is externally
//! positioned by the caller (e.g. `Scaffold.floatingActionButton`); this
//! module inherited the fills-its-box shape from the pre-rework v1 and keeps
//! it, since retrofitting external caller-positioning is out of this task's
//! scope. Items stack upward from the trigger, right-aligned to its trailing
//! edge, closest item first (index `0` sits nearest the trigger) — the
//! *reverse* of upstream's own item-list order (upstream's `Column` lays its
//! `items` top-down with the *last* list entry landing nearest the FAB,
//! `m3e_fab_menu.dart:317-336`); see the Cascade stagger section below for
//! how this reindexing keeps "nearest fires first" true under either
//! convention. [`FAB_TO_ITEMS_GAP`]/[`ITEM_GAP`]/[`ITEM_HEIGHT`]/[`ITEM_ICON`]/
//! [`ITEM_PAD_X`]/[`ITEM_ICON_LABEL_GAP`] are `M3EFabMenuTheme`'s own default
//! geometry tokens (`menuOffset`/`itemGap`/`itemHeight`/`iconSize`/
//! `itemHorizontalPadding`/`iconLabelGap`, `m3e_fab_menu_theme.dart:10-16`),
//! transcribed exactly rather than the pre-rework module's approximations.
//!
//! # Trigger morph (shape + size)
//!
//! On open, the trigger shrinks from [`MAIN_CONTAINER_CLOSED`] (80dp) to
//! [`MAIN_CONTAINER_OPEN`] (56dp) while its outline morphs
//! [`TRIGGER_CLOSED_SHAPE`] (a rounded square) into [`TRIGGER_OPEN_SHAPE`] (a
//! circle, doubling as the visual "close" affordance) — upstream's own class
//! doc: "the FAB shrinks (80→56) and morphs rounded-square ↔ circle"
//! (`m3e_fab_menu.dart:19-24`). Both endpoints are named catalog shapes
//! ([`crate::shapes::ShapeKind::Square`]/[`crate::shapes::ShapeKind::Circle`]), morphed
//! through this crate's [`crate::shapes::Morph`] feature-point engine — the same
//! engine [`mod@super::loading_indicator`] consumes, and the reason
//! [`mod@super::fab`]'s `corner_radius` override seam exists at all
//! (`fab.rs`'s own module doc names this morph as the override's intended
//! consumer). **This module does not call into that seam.** `fab.rs`'s
//! `corner_radius` is a single scalar override on its
//! generic `FabWidget`, which sizes its icon against its own internal
//! [`crate::FabSize`] tier rather than the box it is laid out at —
//! [`mod@super::toolbar`]'s own fab-seam-degrade note documents the resulting
//! off-center icon at a non-native container size, the exact failure mode a
//! trigger that shrinks from 80dp down to a 56dp-native tier would hit.
//! Composing `fab()` would also mean routing pointer input through a second,
//! independent `Widget::event` (its own press/haptics/on_press machinery)
//! underneath this module's own hand-rolled trigger/item/scrim arbitration —
//! a second interaction surface this module does not want. Hand-rolling the
//! trigger's paint instead means every geometry value (container size, icon
//! size, icon origin) is computed from the *actual* current box every frame,
//! so there is no seam to degrade: [`icon_size_for`] scales the icon
//! proportionally with the container (24dp at the 56dp open end, ~34dp at the
//! 80dp closed end — upstream's own `FittedBox` uniformly rescales the whole
//! rendered FAB, icon included, `m3e_fab_menu.dart:257-279`), and layout
//! centers it against whatever box that frame actually laid out.
//!
//! The morph is a **layout** value (the container square itself changes
//! size), not merely a paint value — an in-flight frame calls
//! [`frust::authoring::PaintCtx::request_layout`], the same layout-skip
//! discipline [`mod@super::toolbar`]'s own FAB-morph section documents at
//! length. `Theme::motion.reduce_motion` snaps the morph straight to its
//! resting shape instead of animating through it.
//!
//! The morph spring rings around its closed target the way an item's does
//! (see the Cascade stagger section's retreat clamp below) but is left
//! **unclamped in both directions**: at [`FAB_SHAPE_SPRING`]'s damping its
//! first rebound past zero peaks near `+0.0021`, worth ~0.05px of container
//! size — nothing to see, and this value drives an opaque container's *size*
//! rather than an opacity, so it has no bare-content failure mode to begin
//! with. The measurement is pinned by a test rather than assumed.
//!
//! # Cascade stagger
//!
//! Each item reveals with its own spring ([`ITEM_SPRING`] — upstream's
//! `_expandMotion`,
//! `MaterialSpringMotion.expressiveSpatialDefault().copyWith(damping: 0.55)`,
//! `m3e_fab_menu.dart:84-88` — exactly this crate's own
//! [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`] token), each
//! delayed by [`cascade_delay`] before it launches — upstream's
//! `_expandStaggerMs` (30ms, `m3e_fab_menu.dart:96`) times each item's
//! distance from the FAB (`fromFab = count - 1 - i`,
//! `m3e_fab_menu.dart:192-194`). Reindexed for this module's own item-order
//! convention (index `0` nearest the FAB, the *opposite* of upstream's list
//! order — see the Reveal geometry section above), "nearest fires first"
//! falls straight out of the index itself: item `0` launches immediately,
//! item `n-1` launches `(n-1) * 30ms` later. **Closing reverses the
//! cascade** — the item that revealed last retreats first — a v1 addition,
//! not upstream's own behavior: upstream hides every item *instantly*
//! (`_close`, `m3e_fab_menu.dart:210-226`, snapping every controller to `0`
//! with no animation at all), a simplification of its `OverlayPortal.hide()`
//! unmounting the item subtree outright rather than a deliberate design
//! choice this port preserves. [`FabMenuWidget::advance_cascade`] drives the
//! whole timeline off one shared clock (`cascade_epoch`, seeded from the
//! first `paint` after the cascade starts — a `rebuild` has no frame clock
//! to seed with, the same idiom [`mod@super::loading_indicator`]'s cycle
//! timer uses) rather than real timers, since a retained `Widget` has none to
//! spawn. `Theme::motion.reduce_motion` snaps every item straight to its
//! resting opacity, skipping the stagger entirely.
//!
//! An item's reveal value is an **opacity**, and three rules keep a
//! partly-revealed item from reading as a bare icon+label row with no pill
//! behind it — the failure this component actually shipped:
//!
//! 1. **The item fades as one unit.** Container, icon, and label composite
//!    through a single [`frust::authoring::PaintScene::push_layer`] at the
//!    item's own progress, with the container painted at full alpha inside
//!    it. Fading only the container leaves the two child pods painting
//!    opaque at every progress, so a near-zero-progress item shows its
//!    content over an invisible pill.
//! 2. **A widget built at rest never flings** — [`View::build`] seeds *both*
//!    directions (resting value + `cascade_launched`), not just the open one.
//!    An unseeded closed mount lets the first `paint`'s
//!    [`FabMenuWidget::advance_cascade`] fling every item from rest with the
//!    retreat velocity, which dips below zero and rebounds back above it.
//! 3. **The retreat clamps *and stops* at zero.** [`ITEM_SPRING`] is
//!    under-damped, so a closing item's spring rings around its `0.0` target
//!    instead of stopping at it; [`AnimationController::value_clamped`] hides
//!    only the negative half of that ring, so each rebound re-enters `t > 0`
//!    and flashes the item back on after the collapse has visually settled.
//!    `advance_cascade` snaps a retreating spring to rest the first frame it
//!    reaches zero — Flutter's controllers settle at a bound rather than
//!    oscillating through it. Only the retreat: the reveal's overshoot past
//!    `1.0` is real motion, and the clamped alpha read absorbs it.
//!
//! # Item colors ("contrasting… primary-container family")
//!
//! Each item's container cycles through the three M3 "container" roles —
//! `primary_container`, `secondary_container`, `tertiary_container` (by
//! `index % 3`) — so adjacent items read as visually distinct without a
//! per-item color prop. Item labels use [`ThemeTextColor::OnSurface`]
//! uniformly regardless of which container family the item sits on, the same
//! choice [`mod@super::button_group`] documents (selection/emphasis is conveyed by
//! the container fill, not the label color, to avoid adding a new
//! `on_*_container` text role to `text.rs` — a file outside this module's
//! scope). Every item's shape is a full pill (`ITEM_HEIGHT / 2` radius,
//! [`item_radius`]) — upstream's item container is unconditionally a
//! `StadiumBorder()` (`m3e_fab_menu.dart:382`), never a themed corner-radius
//! token, so this is a fixed geometric fact rather than a `Theme` read.
//! **Not ported**: item elevation (upstream's `itemElevation:
//! M3EElevation.level3`, `m3e_fab_menu_theme.dart:17`) and outline/gradient
//! fills/foregrounds (`itemOutlineColor`/`itemBackgroundGradient`/
//! `itemForegroundGradient`, all unset by default) — a future addition, out
//! of this task's scope (this crate's design-system plugins are
//! `MaterialTokens`-only; no gradient theme extension exists here to carry
//! one).
//!
//! # Scrim
//!
//! The dismiss barrier occupies the whole widget area whenever `open` — no
//! animated fade, matching upstream's own non-animated
//! `_buildDismissBarrier` (mounted/unmounted with the overlay portal,
//! `m3e_fab_menu.dart:305-315`). Its fill color is [`SCRIM_ALPHA`] over
//! `colors.scrim` — upstream's own `M3EFabMenuTheme.scrimOpacity` **defaults
//! to `0.0`** (`m3e_fab_menu_theme.dart:11,80-81`), i.e. the barrier is
//! invisible by default even though it still blocks/dismisses on tap; this
//! is a deliberate divergence from the pre-rework v1's own 32% guess (which
//! matched [`super::sheet`]/[`super::dialog`]'s scrim rather than this
//! component's real upstream default), now transcribed faithfully.
//!
//! # Hit-testing while closed
//!
//! While `open` is `false`, only the trigger's own rect is interactive — a
//! `Down` anywhere else in the widget's (full-area) bounds returns
//! [`EventResult::Ignored`], which `Stack::event`'s reverse-order routing
//! (`frust::authoring::route_event`) then hands to whatever sits below in the Z-order.
//! Once `open` is `true`, every other point in the area is treated as the
//! scrim (dismiss-on-tap) — see `Target::Scrim`.
//!
//! # No `StateLayer` (v1)
//!
//! Unlike [`mod@super::fab`], this widget paints no hover/press `StateLayer`
//! overlay on the trigger or the items — a future addition can mirror
//! `fab.rs`'s.
//!
//! # Keyboard operability
//!
//! **Escape-to-dismiss now works, once the menu has focus.** A `Down` on the
//! trigger, or (while `open`) on an item or the scrim, claims focus via
//! `EventCtx::request_focus` — the same opt-in
//! [`mod@crate::dialog`]/[`crate::sheet`]/
//! `frust_cupertino::alert_dialog`/`frust_cupertino::action_sheet` (the
//! four modal widgets `docs/ARCHITECTURE.md`'s Semantics section names) use;
//! this widget now joins that set. Once focused, a focus-routed
//! `Key(Escape)` fires the same `on_toggle` callback a scrim/item/trigger tap
//! would, but only while `open` — a closed menu has nothing to dismiss, so
//! Escape is ignored outright regardless of focus. As with the other four
//! modal widgets, there is still no hook to auto-focus the menu on open; a
//! caller must complete one pointer interaction with the open menu before
//! Escape does anything.

use std::sync::OnceLock;
use std::time::Duration;

use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{AnimationController, FrameTime, SpringDesc, Theme};
use kurbo::{Affine, BezPath, Point, Rect, Size};
use peniko::{Brush, Color};

use frust::authoring::{ThemeTextColor, ThemeTextType};

use super::press::presses;
use crate::shapes::{Morph, ShapeKind};
use crate::tokens::MaterialSpring;

/// Trigger container size when the menu is closed, in logical px — upstream's
/// `M3EFabMenuTheme.closedFabContainer` default (`m3e_fab_menu_theme.dart:18,58`).
const MAIN_CONTAINER_CLOSED: f64 = 80.0;
/// Trigger container size when the menu is open, in logical px — upstream's
/// `M3EFabMenuTheme.openFabContainer` default (`m3e_fab_menu_theme.dart:19,61`),
/// matching [`super::fab`]'s medium-tier `MEDIUM_CONTAINER`.
const MAIN_CONTAINER_OPEN: f64 = 56.0;
/// Icon size inside the trigger FAB at the fully-*open* end of the morph, in
/// logical px — matches [`super::fab`]'s `MEDIUM_ICON`. [`icon_size_for`]
/// scales this up toward the closed end, proportionally with the container —
/// see the [module docs](self)' Trigger morph section.
const MAIN_ICON: f64 = 24.0;

/// Distance from the host area's right/bottom edges to the trigger FAB, in
/// logical px.
///
/// **Community-approximate**: a FAB's screen-edge margin is a common M3
/// layout convention (16dp), not an independently-verified published value.
const EDGE_MARGIN: f64 = 16.0;

/// Fixed height of a menu-item row, in logical px — upstream's
/// `M3EFabMenuTheme.itemHeight` default (`m3e_fab_menu_theme.dart:13,43`).
const ITEM_HEIGHT: f64 = 56.0;
/// Icon size inside a menu-item row, in logical px — upstream's
/// `M3EFabMenuTheme.iconSize` default (`m3e_fab_menu_theme.dart:15,49`).
const ITEM_ICON: f64 = 24.0;
/// Horizontal padding inside a menu-item row, in logical px — upstream's
/// `M3EFabMenuTheme.itemHorizontalPadding` default
/// (`m3e_fab_menu_theme.dart:14,46`).
const ITEM_PAD_X: f64 = 20.0;
/// Gap between a menu-item's icon and its label, in logical px — upstream's
/// `M3EFabMenuTheme.iconLabelGap` default (`m3e_fab_menu_theme.dart:16,51`).
const ITEM_ICON_LABEL_GAP: f64 = 12.0;
/// Vertical gap between two stacked menu items, in logical px — upstream's
/// `M3EFabMenuTheme.itemGap` default (`m3e_fab_menu_theme.dart:12,40`).
const ITEM_GAP: f64 = 12.0;
/// Vertical gap between the trigger FAB and the nearest (index `0`) menu item,
/// in logical px — upstream's `M3EFabMenuTheme.menuOffset` default
/// (`m3e_fab_menu_theme.dart:10,31-34`).
const FAB_TO_ITEMS_GAP: f64 = 12.0;

/// Unthemed-fallback trigger-FAB container fill (a theme resolves
/// `colors.primary_container`) — matches [`super::fab`]'s `CONTAINER`.
const MAIN_CONTAINER_COLOR: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);

/// Unthemed-fallback item container fill, `index % 3 == 0` family (a theme
/// resolves `colors.primary_container`) — matches `color_scheme_light`'s
/// light default.
const PRIMARY_CONTAINER: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);
/// Unthemed-fallback item container fill, `index % 3 == 1` family (a theme
/// resolves `colors.secondary_container`).
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback item container fill, `index % 3 == 2` family (a theme
/// resolves `colors.tertiary_container`).
const TERTIARY_CONTAINER: Color = Color::from_rgb8(0xFF, 0xD8, 0xE4);

/// Scrim opacity behind the open menu — upstream's `M3EFabMenuTheme.
/// scrimOpacity` default (`m3e_fab_menu_theme.dart:11,80-81`). See the
/// [module docs](self)' Scrim section for why this is `0.0`.
const SCRIM_ALPHA: f32 = 0.0;
/// Unthemed-fallback scrim base color (a theme resolves `colors.scrim`).
const SCRIM_FALLBACK: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Unthemed-fallback shadow y-offset for the trigger FAB, matching
/// `crate::tokens::elevation().level3`'s `y_offset` exactly — matches
/// [`super::fab`]'s `FALLBACK_SHADOW_Y_OFFSET`.
const FALLBACK_SHADOW_Y_OFFSET: f64 = 4.0;
/// Unthemed-fallback shadow blur std-dev — matches [`super::fab`]'s
/// `FALLBACK_SHADOW_BLUR`.
const FALLBACK_SHADOW_BLUR: f64 = 6.0;
/// Unthemed-fallback shadow color — matches [`super::fab`]'s
/// `FALLBACK_SHADOW_COLOR`.
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The trigger's closed-state outline — see the [module docs](self)'
/// Trigger morph section.
const TRIGGER_CLOSED_SHAPE: ShapeKind = ShapeKind::Square;
/// The trigger's open-state outline — see the [module docs](self)' Trigger
/// morph section.
const TRIGGER_OPEN_SHAPE: ShapeKind = ShapeKind::Circle;

/// Closed-end corner-radius fraction of the container size, for the
/// shadow's own scalar `radius` parameter only — [`PaintScene::draw_shadow`]
/// takes a rounded-rect radius, not a path, so the true
/// [`TRIGGER_CLOSED_SHAPE`]/[`TRIGGER_OPEN_SHAPE`] outline the fill paints
/// has no exact shadow counterpart; this is a shadow-only stand-in
/// (matching the pre-rework module's own `shape.large`-derived 16/56
/// fraction). Shadows blur enough that the approximation is imperceptible.
const SHADOW_RADIUS_FRACTION_CLOSED: f64 = 16.0 / 56.0;

/// The trigger's shape/size morph spring — upstream's `_fabShapeMotion`:
/// `MaterialSpringMotion.expressiveSpatialDefault().copyWith(damping: 0.7)`
/// (`m3e_fab_menu.dart:90-94`) —
/// [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`]'s stiffness
/// (380) with the damping ratio overridden to `0.7`.
const FAB_SHAPE_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: 0.7,
};

/// The per-item cascade-reveal spring — upstream's `_expandMotion`:
/// `MaterialSpringMotion.expressiveSpatialDefault().copyWith(damping: 0.55)`
/// (`m3e_fab_menu.dart:84-88`), which is exactly this crate's own
/// [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`] token.
const ITEM_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_PRESS.stiffness,
    damping_ratio: MaterialSpring::EXPRESSIVE_SPATIAL_PRESS.damping_ratio,
};

/// Per-item cascade stagger step, in milliseconds — upstream's
/// `_expandStaggerMs` (`m3e_fab_menu.dart:96`).
const EXPAND_STAGGER_MS: u64 = 30;

/// Nominal period seeding a spring-driven [`AnimationController`]; the
/// motion is spring-driven via `fling`, so this only backs construction.
const ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (value-units/sec) for every spring `fling` in this module
/// (the trigger morph and each item's own cascade spring) — a named nudge
/// rather than a literal `0`, the same convention [`super::fab`]'s
/// `PRESS_SPRING`/[`super::button_group`]'s `SQUISH_SPRING` launches use.
const FLING_VELOCITY: f64 = 3.0;

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Linear interpolation from `a` to `b` at `t` (unclamped — callers clamp at
/// their own use site, matching [`frust::AnimationController::value`]'s own
/// overshoot contract).
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Coerce a possibly-infinite constraint dimension to a finite value (this
/// widget fills its area like [`super::sheet`] — a navigator page or a
/// full-screen [`frust::Stack`] gives it bounded constraints).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The trigger icon's size at morph progress `t`'s container size —
/// proportional scaling with the container, upstream's own `FittedBox`
/// uniformly rescaling the whole rendered FAB (icon included) — see the
/// [module docs](self)' Trigger morph section.
fn icon_size_for(container: f64) -> f64 {
    MAIN_ICON * (container / MAIN_CONTAINER_OPEN)
}

/// The trigger's shadow radius at morph progress `t` — see
/// [`SHADOW_RADIUS_FRACTION_CLOSED`]'s doc for why this is an approximation
/// rather than the true morphed outline.
fn trigger_shadow_radius(container: f64, t: f64) -> f64 {
    let closed = container * SHADOW_RADIUS_FRACTION_CLOSED;
    let open = container / 2.0;
    lerp(closed, open, t.clamp(0.0, 1.0))
}

/// The trigger's closed↔open outline morph, built once and shared — see
/// [`Morph`]'s own cost note.
fn trigger_morph() -> &'static Morph {
    static CACHE: OnceLock<Morph> = OnceLock::new();
    CACHE.get_or_init(|| {
        Morph::new(
            TRIGGER_CLOSED_SHAPE.polygon().clone(),
            TRIGGER_OPEN_SHAPE.polygon().clone(),
        )
    })
}

/// The trigger's outline at morph progress `t` (clamped to `0.0..=1.0` — see
/// [`Morph::path`]'s overshoot caveat), scaled and positioned to exactly
/// fill `rect` (local widget coordinates, e.g. [`FabMenuWidget::fab_rect`]).
/// Every catalog shape's raw outline is already normalized into
/// `(0,0)..(1,1)` (the same fact [`mod@super::loading_indicator`]'s Geometry
/// section documents), so this is a plain non-uniform scale plus translate —
/// no rotation, no re-centering.
fn trigger_path(t: f64, rect: Rect) -> BezPath {
    let raw = trigger_morph().path(t.clamp(0.0, 1.0));
    let scale = Affine::new([rect.width(), 0.0, 0.0, rect.height(), 0.0, 0.0]);
    Affine::translate(rect.origin().to_vec2()) * (scale * raw)
}

/// Item `i`'s cascade start delay for the current direction, given `count`
/// total items — see the [module docs](self)' Cascade stagger section.
fn cascade_delay(i: usize, count: usize, opening: bool) -> Duration {
    let step = if opening {
        i
    } else {
        count.saturating_sub(1).saturating_sub(i)
    };
    Duration::from_millis(step as u64 * EXPAND_STAGGER_MS)
}

/// Cancel any in-flight motion on `anim` and jump it straight to `open`'s
/// resting value (`1.0`/`0.0`) — the `Theme.motion.reduce_motion` snap path,
/// and the seed a widget built already-open uses to settle with no
/// animate-in frame. `now` is only a clock-seed for
/// [`AnimationController::advance`]'s contract; a zero-`Duration` motion
/// resolves regardless of the delta it computes, so any value (including
/// [`FrameTime::ZERO`]) is safe here. Returns whether this actually moved
/// the value.
fn snap_anim(anim: &mut AnimationController, open: bool, now: FrameTime) -> bool {
    let target = if open { 1.0 } else { 0.0 };
    let moved = anim.value() != target;
    *anim = AnimationController::new(Duration::ZERO);
    if open {
        anim.forward();
    } else {
        anim.reverse();
    }
    anim.advance(now);
    moved
}

/// The trigger-FAB container fill. Themed: `colors.primary_container`.
/// Unthemed: [`MAIN_CONTAINER_COLOR`].
fn resolve_main_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().primary_container,
        None => MAIN_CONTAINER_COLOR,
    }
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters at M3
/// elevation level 3 — identical resolution to [`super::fab`]'s.
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = theme.elevation.level3;
            let shadow = level.shadow(theme.brightness);
            let color = with_alpha(theme.scheme().shadow, shadow.color_alpha);
            (shadow.blur_std_dev, shadow.y_offset, color)
        }
        None => (
            FALLBACK_SHADOW_BLUR,
            FALLBACK_SHADOW_Y_OFFSET,
            FALLBACK_SHADOW_COLOR,
        ),
    }
}

/// The resolved scrim fill ([`SCRIM_ALPHA`] over `colors.scrim`). Themed:
/// `colors.scrim`.
fn resolve_scrim_base(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().scrim,
        None => SCRIM_FALLBACK,
    }
}

/// The `index`-th item's container fill, cycling `primary_container` →
/// `secondary_container` → `tertiary_container` (`index % 3`). Themed: the
/// matching `colors.*_container` role. Unthemed: [`PRIMARY_CONTAINER`]/
/// [`SECONDARY_CONTAINER`]/[`TERTIARY_CONTAINER`].
fn resolve_item_color(theme: Option<&Theme>, index: usize) -> Color {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            match index % 3 {
                0 => s.primary_container,
                1 => s.secondary_container,
                _ => s.tertiary_container,
            }
        }
        None => match index % 3 {
            0 => PRIMARY_CONTAINER,
            1 => SECONDARY_CONTAINER,
            _ => TERTIARY_CONTAINER,
        },
    }
}

/// An item's corner radius — always a full pill (`ITEM_HEIGHT / 2`), never a
/// themed token. See the [module docs](self)' Item colors section.
fn item_radius() -> f64 {
    ITEM_HEIGHT / 2.0
}

/// Build a menu item's visible label view, themed [`ThemeTextColor::OnSurface`]
/// — see the [module docs](self) for why every item uses this role uniformly —
/// its family following the live theme's `titleMedium` role (the M3 FAB menu
/// item label token).
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    frust::authoring::any::<State, _>(
        frust::text(label)
            .themed_role(ThemeTextColor::OnSurface)
            .themed_family(ThemeTextType::TitleMedium),
    )
}

/// Which interactive region an in-flight press targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The trigger FAB.
    Fab,
    /// The `usize`-th menu item (only reachable while `open`).
    Item(usize),
    /// Anywhere else in the widget's area while `open` — the modal scrim.
    Scrim,
}

/// One item's icon + label + selection callback. Built with [`fab_menu_item`].
pub struct FabMenuItem<State: 'static> {
    icon: AnyView<State>,
    label: String,
    on_select: std::rc::Rc<dyn Fn(&mut State)>,
}

/// Create a FAB-menu item: `icon` + `label` (both painted in the item's row —
/// see the [module docs](self) for the item's fixed row anatomy), running
/// `on_select` when tapped (followed by the menu's own `on_toggle`, requesting
/// a close — see [`fab_menu`]'s docs).
pub fn fab_menu_item<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    label: impl Into<String>,
    on_select: F,
) -> FabMenuItem<State> {
    FabMenuItem {
        icon,
        label: label.into(),
        on_select: std::rc::Rc::new(on_select),
    }
}

/// A declarative M3X FAB menu. See the [module docs](self).
pub struct FabMenuView<State: 'static> {
    icon: AnyView<State>,
    /// The controlled expanded state — see the [module docs](self).
    open: bool,
    items: Vec<FabMenuItem<State>>,
    /// The accessible name for the trigger button (both open/closed states).
    label: Option<String>,
    on_toggle: std::rc::Rc<dyn Fn(&mut State)>,
}

/// Create a FAB menu with a `icon` trigger, the controlled `open` flag, and
/// `items` (built with [`fab_menu_item`]). `on_toggle` fires with `&mut State`
/// on every gesture requesting an open/close flip — see the
/// [module docs](self) for the full list of such gestures, and chain
/// [`FabMenuView::label`] to set the trigger's accessible name (defaults to
/// `"Menu"`).
pub fn fab_menu<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    open: bool,
    items: Vec<FabMenuItem<State>>,
    on_toggle: F,
) -> FabMenuView<State> {
    FabMenuView {
        icon,
        open,
        items,
        label: None,
        on_toggle: std::rc::Rc::new(on_toggle),
    }
}

/// PascalCase alias for [`fab_menu`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn FabMenu<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    open: bool,
    items: Vec<FabMenuItem<State>>,
    on_toggle: F,
) -> FabMenuView<State> {
    fab_menu(icon, open, items, on_toggle)
}

impl<State: 'static> FabMenuView<State> {
    /// Set the trigger's accessible name (both open/closed states).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The retained per-item state: icon + label pods, plus the erased callback.
struct FabMenuItemPod {
    icon: ChildPod,
    label: ChildPod,
    on_select: frust::authoring::ErasedCallback,
}

impl frust::authoring::VisitPods for FabMenuItemPod {
    fn visit_pods(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        visitor(&self.icon);
        visitor(&self.label);
    }
}

/// The retained widget for a [`FabMenuView`]. See the [module docs](self).
pub struct FabMenuWidget {
    icon: ChildPod,
    items: Vec<FabMenuItemPod>,
    /// Retained item label strings, for rebuild diffing + semantics nodes.
    item_labels: Vec<String>,
    /// The controlled expanded state (never self-mutated).
    open: bool,
    /// The accessible trigger label, `"Menu"` when unset.
    label: Option<String>,
    /// The trigger's shape/size morph spring (`0.0` closed, `1.0` open) —
    /// upstream's `_fabShapeCtrl`. Drives a *layout* value — see the
    /// [module docs](self)' Trigger morph section.
    fab_morph: AnimationController,
    /// Each item's own cascade-reveal spring — upstream's `_itemCtrls`.
    item_anim: Vec<AnimationController>,
    /// Whether item `i`'s own fling has launched for the in-flight cascade —
    /// see [`cascade_delay`]/[`Self::advance_cascade`].
    cascade_launched: Vec<bool>,
    /// Frame time the in-flight cascade began, seeded on the first `paint`
    /// after [`Self::drive_reveal`] (a `rebuild` has no frame clock to seed
    /// with) — `None` means "not yet seeded".
    cascade_epoch: Option<FrameTime>,
    /// The trigger's rect, in local space, from [`Widget::layout`].
    fab_rect: Rect,
    /// Each item's rect, in local space, index `0` nearest the trigger.
    item_rects: Vec<Rect>,
    /// The armed target of an in-flight press (`None` when idle).
    armed: Option<Target>,
    /// Whether the armed pointer is currently inside the armed target's rect
    /// (always `true` for [`Target::Scrim`] — see [`Widget::event`]).
    pressed_inside: bool,
    on_toggle: frust::authoring::ErasedCallback,
}

impl FabMenuWidget {
    /// (Re)launch the trigger morph toward the current `open` target and
    /// (re)arm the item cascade — see the [module docs](self)' Cascade
    /// stagger section. Each item's own spring continues from wherever it
    /// currently sits (interrupting an in-flight cascade reverses smoothly
    /// rather than snapping); only the stagger *timeline* (launch delays,
    /// launched-flags, epoch) resets.
    fn drive_reveal(&mut self) {
        let velocity = if self.open {
            FLING_VELOCITY
        } else {
            -FLING_VELOCITY
        };
        self.fab_morph.fling(velocity, FAB_SHAPE_SPRING);
        self.cascade_launched = vec![false; self.item_anim.len()];
        self.cascade_epoch = None;
    }

    /// Advance the cascade timeline to frame time `now`: launches any item
    /// whose stagger delay has elapsed and hasn't fired yet (continuing from
    /// its current value), then advances every item spring, stopping a
    /// *retreating* one dead at `0.0` (see the [module docs](self)' Cascade
    /// stagger section). Returns whether anything moved (paint's "needs
    /// another frame" signal).
    fn advance_cascade(&mut self, now: FrameTime) -> bool {
        let epoch = *self.cascade_epoch.get_or_insert(now);
        let elapsed = now.saturating_sub(epoch);
        let n = self.item_anim.len();
        let mut moved = false;
        for i in 0..n {
            if !self.cascade_launched[i] && elapsed >= cascade_delay(i, n, self.open) {
                let velocity = if self.open {
                    FLING_VELOCITY
                } else {
                    -FLING_VELOCITY
                };
                self.item_anim[i].fling(velocity, ITEM_SPRING);
                self.cascade_launched[i] = true;
            }
            moved |= self.item_anim[i].advance(now);
            // Retreat clamp: [`ITEM_SPRING`] is under-damped, so a closing
            // item's spring rings *around* its `0.0` target rather than
            // stopping at it. `value_clamped` hides only the negative half of
            // that ring — every rebound re-enters `t > 0` and flashes the item
            // back on. Stop it at the bound the first frame it gets there, the
            // way Flutter's own controllers settle at a bound instead of
            // oscillating through it. The reveal direction is deliberately not
            // clamped: its overshoot past `1.0` is real motion the alpha read
            // already absorbs.
            if !self.open && self.item_anim[i].is_animating() && self.item_anim[i].value() <= 0.0 {
                snap_anim(&mut self.item_anim[i], false, now);
            }
        }
        moved
    }
}

impl<State: 'static> View<State> for FabMenuView<State> {
    type Element = FabMenuWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FabMenuWidget {
        let items = self
            .items
            .iter()
            .map(|item| FabMenuItemPod {
                icon: frust::authoring::build_child(&item.icon, ctx),
                label: frust::authoring::build_child(&label_view::<State>(item.label.clone()), ctx),
                on_select: frust::authoring::erase_callback(&item.on_select),
            })
            .collect();

        let n = self.items.len();
        let mut fab_morph = AnimationController::new(ANIM_PERIOD);
        let mut item_anim: Vec<AnimationController> = (0..n)
            .map(|_| AnimationController::new(ANIM_PERIOD))
            .collect();
        // Seed the resting state in *both* directions (see the [`snap_anim`]
        // doc): a menu built already-open shows its items without needing a
        // frame to spring into them, and a menu built closed is marked
        // fully-launched so it never flings at all — see the [module docs](self)'
        // Cascade stagger section for why an unseeded closed mount blinks.
        snap_anim(&mut fab_morph, self.open, FrameTime::ZERO);
        for a in item_anim.iter_mut() {
            snap_anim(a, self.open, FrameTime::ZERO);
        }
        let cascade_launched = vec![true; n];

        FabMenuWidget {
            icon: frust::authoring::build_child(&self.icon, ctx),
            items,
            item_labels: self.items.iter().map(|i| i.label.clone()).collect(),
            open: self.open,
            label: self.label.clone(),
            fab_morph,
            item_anim,
            cascade_launched,
            cascade_epoch: None,
            fab_rect: Rect::ZERO,
            item_rects: Vec::new(),
            armed: None,
            pressed_inside: false,
            on_toggle: frust::authoring::erase_callback(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FabMenuWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = frust::authoring::erase_callback(&self.on_toggle);
        let mut flags = ChangeFlags::NONE;

        flags |= frust::authoring::rebuild_child(&prev.icon, &self.icon, &mut element.icon, ctx);

        if prev.open != self.open {
            element.open = self.open;
            element.drive_reveal();
            // A closed-triggered rebuild invalidates any in-flight item/scrim
            // press — nothing left to release onto.
            if !self.open {
                element.armed = None;
                element.pressed_inside = false;
            }
            flags |= ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            element.label = self.label.clone();
        }

        if prev.items.len() != self.items.len() {
            // Structural change: tear down every old item pod, build fresh
            // ones for the new list (mirrors `super::fab`'s icon/label
            // teardown-then-rebuild path for a structural swap).
            for (old_item, pod) in prev.items.iter().zip(element.items.iter_mut()) {
                frust::authoring::teardown_child(&old_item.icon, &mut pod.icon, ctx);
                let old_label_view = label_view::<State>(old_item.label.clone());
                frust::authoring::teardown_child(&old_label_view, &mut pod.label, ctx);
            }
            element.items = self
                .items
                .iter()
                .map(|item| FabMenuItemPod {
                    icon: frust::authoring::build_child(&item.icon, ctx),
                    label: frust::authoring::build_child(
                        &label_view::<State>(item.label.clone()),
                        ctx,
                    ),
                    on_select: frust::authoring::erase_callback(&item.on_select),
                })
                .collect();
            element.item_labels = self.items.iter().map(|i| i.label.clone()).collect();
            element.armed = None;
            element.pressed_inside = false;
            // New item count: fresh cascade state, matching upstream's own
            // `didUpdateWidget` (`m3e_fab_menu.dart:123-136`) — every item
            // settles instantly at the *current* open state, no animate-in.
            let n = self.items.len();
            element.item_anim = (0..n)
                .map(|_| AnimationController::new(ANIM_PERIOD))
                .collect();
            element.cascade_epoch = None;
            // Both directions, exactly like `build`'s own seed: a fresh item
            // set mounted while *closed* must rest at zero rather than fling
            // toward it.
            for a in element.item_anim.iter_mut() {
                snap_anim(a, element.open, FrameTime::ZERO);
            }
            element.cascade_launched = vec![true; n];
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (i, (prev_item, next_item)) in prev.items.iter().zip(self.items.iter()).enumerate()
            {
                let pod = &mut element.items[i];
                flags |= frust::authoring::rebuild_child(
                    &prev_item.icon,
                    &next_item.icon,
                    &mut pod.icon,
                    ctx,
                );
                if prev_item.label != next_item.label {
                    let prev_view = label_view::<State>(prev_item.label.clone());
                    let next_view = label_view::<State>(next_item.label.clone());
                    flags |= frust::authoring::rebuild_child(
                        &prev_view,
                        &next_view,
                        &mut pod.label,
                        ctx,
                    );
                    element.item_labels[i] = next_item.label.clone();
                }
                pod.on_select = frust::authoring::erase_callback(&next_item.on_select);
            }
        }

        flags
    }

    fn teardown(&self, element: &mut FabMenuWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.icon, &mut element.icon, ctx);
        for (item, pod) in self.items.iter().zip(element.items.iter_mut()) {
            frust::authoring::teardown_child(&item.icon, &mut pod.icon, ctx);
            let label_view = label_view::<State>(item.label.clone());
            frust::authoring::teardown_child(&label_view, &mut pod.label, ctx);
        }
    }
}

impl Widget for FabMenuWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        // Trigger FAB: bottom-right anchored, sized by the morph's current
        // value — see the [module docs](self)' Trigger morph section.
        let fab_t = self.fab_morph.value_clamped();
        let container = lerp(MAIN_CONTAINER_CLOSED, MAIN_CONTAINER_OPEN, fab_t);
        let icon_size = icon_size_for(container);

        let icon_layout_size = self
            .icon
            .layout_child(ctx, &BoxConstraints::tight(Size::new(icon_size, icon_size)));
        let fab_origin = Point::new(
            (area_w - EDGE_MARGIN - container).max(0.0),
            (area_h - EDGE_MARGIN - container).max(0.0),
        );
        self.fab_rect = Rect::from_origin_size(fab_origin, Size::new(container, container));
        self.icon.set_origin(Point::new(
            fab_origin.x + (container - icon_layout_size.width) / 2.0,
            fab_origin.y + (container - icon_layout_size.height) / 2.0,
        ));

        // Items stack upward from the trigger, right-aligned to its trailing
        // edge, index 0 nearest the trigger.
        self.item_rects.clear();
        let right_edge = self.fab_rect.x1;
        let mut bottom = self.fab_rect.y0 - FAB_TO_ITEMS_GAP;
        let radius = item_radius();
        for pod in self.items.iter_mut() {
            let icon_size = pod
                .icon
                .layout_child(ctx, &BoxConstraints::tight(Size::new(ITEM_ICON, ITEM_ICON)));
            let label_size = pod.label.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, ITEM_HEIGHT)),
            );
            let content_w = icon_size.width + ITEM_ICON_LABEL_GAP + label_size.width;
            let item_w = (content_w + ITEM_PAD_X * 2.0).max(radius * 2.0);
            let top = bottom - ITEM_HEIGHT;
            let rect = Rect::new((right_edge - item_w).max(0.0), top, right_edge, bottom);

            pod.icon.set_origin(Point::new(
                rect.x0 + ITEM_PAD_X,
                rect.y0 + (ITEM_HEIGHT - icon_size.height) / 2.0,
            ));
            pod.label.set_origin(Point::new(
                rect.x0 + ITEM_PAD_X + icon_size.width + ITEM_ICON_LABEL_GAP,
                rect.y0 + (ITEM_HEIGHT - label_size.height) / 2.0,
            ));

            self.item_rects.push(rect);
            bottom = top - ITEM_GAP;
        }

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Resolve every theme-derived value up front — `theme` borrows `ctx`
        // immutably, which would otherwise conflict with the mutable
        // `pod.*.paint_child(ctx, ...)` calls and `ctx.request_layout()`
        // below.
        let theme = Theme::from_paint_ctx(ctx);
        let scrim_base = resolve_scrim_base(theme);
        let item_colors: Vec<Color> = (0..self.items.len())
            .map(|i| resolve_item_color(theme, i))
            .collect();
        let main_color = resolve_main_color(theme);
        let (blur, y_offset, shadow_color) = resolve_shadow(theme);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let now = ctx.frame_time();

        // The trigger morph drives a *layout* value (the container square),
        // so an in-flight frame needs an explicit relayout, not merely
        // another frame — the layout-skip class `mod@super::toolbar`'s own
        // FAB-morph section documents. The settling frame both stops
        // animating *and* lands on the target, so gate on "did the value
        // move" rather than the animating flag alone.
        let fab_moved = if reduce_motion {
            snap_anim(&mut self.fab_morph, self.open, now)
        } else {
            let before = self.fab_morph.value();
            let animating = self.fab_morph.advance(now);
            animating || self.fab_morph.value() != before
        };
        if fab_moved {
            ctx.request_layout();
        }

        // The item cascade is a paint-only value (opacity) — see the
        // [module docs](self)' Cascade stagger section.
        let n = self.item_anim.len();
        if reduce_motion {
            for a in self.item_anim.iter_mut() {
                snap_anim(a, self.open, now);
            }
            self.cascade_launched = vec![true; n];
        } else {
            self.advance_cascade(now);
        }

        let origin = ctx.origin();

        if self.open {
            let scrim = with_alpha(scrim_base, SCRIM_ALPHA);
            scene.fill_rect(origin, ctx.size(), scrim);
        }

        let item_radius = item_radius();
        for (i, (pod, rect)) in self
            .items
            .iter_mut()
            .zip(self.item_rects.iter())
            .enumerate()
        {
            let t = self.item_anim[i].value_clamped();
            if t <= 0.0 {
                continue;
            }
            // The item fades as ONE unit — container, icon, and label
            // composited through a single layer at the item's own progress,
            // with the container itself painted at full alpha inside it.
            // Fading only the container (the pods paint opaque regardless)
            // is what made a low-`t` item read as a bare icon+label row with
            // no pill behind it. See the [module docs](self)' Cascade stagger
            // section.
            let item_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            scene.push_layer(item_origin, rect.size(), t as f32);
            scene.fill_rounded_rect(item_origin, rect.size(), item_radius, item_colors[i]);
            pod.icon.paint_child(ctx, scene);
            pod.label.paint_child(ctx, scene);
            scene.pop_layer();
        }

        // Trigger FAB: always painted, on top of the scrim/items — the
        // shape morph itself, see the [module docs](self)' Trigger morph
        // section.
        let fab_t = self.fab_morph.value_clamped();
        let shadow_radius = trigger_shadow_radius(self.fab_rect.width(), fab_t);
        scene.draw_shadow(
            Point::new(
                origin.x + self.fab_rect.x0,
                origin.y + self.fab_rect.y0 + y_offset,
            ),
            self.fab_rect.size(),
            shadow_radius,
            blur,
            shadow_color,
        );
        let path = trigger_path(fab_t, self.fab_rect);
        scene.fill_path(origin, &path, &Brush::Solid(main_color));
        self.icon.paint_child(ctx, scene);

        if !reduce_motion {
            let still_animating = self.fab_morph.is_animating()
                || self.item_anim.iter().any(AnimationController::is_animating)
                || self.cascade_launched.iter().any(|launched| !launched);
            if still_animating {
                ctx.request_frame();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Escape (once focused) requests a close through the same `on_toggle`
        // path a scrim/item/trigger tap uses — but only while open (see the
        // module docs' Keyboard operability note); a closed menu has nothing
        // to dismiss, so Escape is ignored outright regardless of focus.
        if let InputEvent::Key(key_event) = event {
            if self.open && key_event.key == Key::Named(NamedKey::Escape) {
                (self.on_toggle)(ctx);
                ctx.request_redraw();
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if self.fab_rect.contains(p.position) {
                    // Claim focus on every trigger press (see the module
                    // docs' Keyboard operability note). Load-bearing: the
                    // root treats a Down that bubbles no claim as a blur
                    // (`release_focus_session` drops focus + IME state), so
                    // the re-claim is what keeps the session alive while this
                    // menu is up. Re-claiming while already focused is a
                    // change-guarded no-op (no generation bump) — do not add
                    // a claim-once guard, it kills the session on the second
                    // tap (claim-once-hygiene review, 2026-08-06).
                    ctx.request_focus();
                    // Only a primary press arms the trigger. A secondary press
                    // is a context gesture: still swallowed while the menu is
                    // open (its scrim is a modal barrier), simply ignored while
                    // it is closed.
                    if !presses(p) {
                        return if self.open {
                            EventResult::Handled
                        } else {
                            EventResult::Ignored
                        };
                    }
                    self.armed = Some(Target::Fab);
                    self.pressed_inside = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if !self.open {
                    return EventResult::Ignored;
                }
                // Same opt-in while open: a press on an item or the scrim
                // also claims focus on every Down — see the trigger arm's
                // comment above for why.
                ctx.request_focus();
                // The open menu's scrim is a modal barrier: it swallows a
                // secondary press like any other, but arms neither an item nor
                // a scrim dismiss from it.
                if !presses(p) {
                    return EventResult::Handled;
                }
                if let Some(i) = self.item_rects.iter().position(|r| r.contains(p.position)) {
                    self.armed = Some(Target::Item(i));
                } else {
                    self.armed = Some(Target::Scrim);
                }
                self.pressed_inside = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(target) = self.armed else {
                    return EventResult::Ignored;
                };
                self.pressed_inside = match target {
                    Target::Fab => self.fab_rect.contains(p.position),
                    Target::Item(i) => self
                        .item_rects
                        .get(i)
                        .is_some_and(|r| r.contains(p.position)),
                    // The scrim dismiss fires on any release, wherever the
                    // gesture ends up — see the module docs.
                    Target::Scrim => true,
                };
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(target) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if self.pressed_inside {
                    match target {
                        Target::Fab => (self.on_toggle)(ctx),
                        Target::Item(i) => {
                            if let Some(pod) = self.items.get_mut(i) {
                                (pod.on_select)(ctx);
                            }
                            (self.on_toggle)(ctx);
                        }
                        Target::Scrim => (self.on_toggle)(ctx),
                    }
                }
                self.pressed_inside = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                self.armed = None;
                self.pressed_inside = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let trigger_label = self.label.as_deref().unwrap_or("Menu");
        if self.open {
            ctx.push_container(
                Role::Group,
                |_| {},
                |ctx| {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(trigger_label);
                        node.set_expanded(true);
                        node.add_action(Action::Click);
                        node.add_action(Action::Expand);
                    });
                    ctx.push_container(
                        Role::Menu,
                        |_| {},
                        |ctx| {
                            for label in &self.item_labels {
                                ctx.push_node(Role::MenuItem, |node| {
                                    node.set_label(label.as_str());
                                    node.add_action(Action::Click);
                                });
                            }
                        },
                    );
                },
            );
        } else {
            ctx.push_node(Role::Button, |node| {
                node.set_label(trigger_label);
                node.set_expanded(false);
                node.add_action(Action::Click);
                node.add_action(Action::Expand);
            });
        }
    }

    frust::authoring::visit_children!(items, icon);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{KeyEvent, Modifiers, PointerButton, PointerEvent};
    use frust_core::RenderRoot;
    use frust_widgets::test_support::leaf_any;
    use kurbo::{PathEl, Shape};
    use std::any::Any;

    /// A minimal `State`-generic icon stand-in (mirrors `fab.rs`'s
    /// `icon_stub`: `test_support::leaf_any` only implements `View<()>`, but
    /// this module's tests need a generic `State` — a plain themed
    /// [`frust::text`] run stands in for the icon, its content
    /// irrelevant to firing behavior).
    fn icon_stub<State: 'static>() -> AnyView<State> {
        frust::authoring::any::<State, _>(frust::text("i"))
    }

    #[derive(Default)]
    struct Log {
        toggles: u32,
        selected: Vec<usize>,
    }

    fn item(i: usize) -> FabMenuItem<Log> {
        fab_menu_item(
            icon_stub::<Log>(),
            format!("Item {i}"),
            move |s: &mut Log| s.selected.push(i),
        )
    }

    fn build_menu(open: bool, item_count: usize) -> FabMenuWidget {
        let view = fab_menu::<Log, _>(
            icon_stub::<Log>(),
            open,
            (0..item_count).map(item).collect(),
            |s: &mut Log| s.toggles += 1,
        );
        let mut counter = 0u64;
        View::<Log>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    /// Directly snap every spring to the fully-open resting state, bypassing
    /// `paint`'s own stagger/spring advance entirely — the deterministic
    /// route to an "everything settled open" fixture a real spring can only
    /// approach asymptotically. Mirrors [`snap_anim`]'s own reduce-motion
    /// path.
    fn open_fully(w: &mut FabMenuWidget) {
        snap_anim(&mut w.fab_morph, true, FrameTime::ZERO);
        for a in w.item_anim.iter_mut() {
            snap_anim(a, true, FrameTime::ZERO);
        }
        w.cascade_launched = vec![true; w.item_anim.len()];
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut FabMenuWidget, state: &mut Log, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, event)
    }

    fn laid_out(w: &mut FabMenuWidget) {
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));
    }

    // -----------------------------------------------------------------------
    // Trigger shape morph (acceptance: endpoints cited, mid-t sanity).
    // -----------------------------------------------------------------------

    #[test]
    fn trigger_shape_endpoints_are_a_rounded_square_and_a_circle() {
        assert_eq!(TRIGGER_CLOSED_SHAPE, ShapeKind::Square);
        assert_eq!(TRIGGER_OPEN_SHAPE, ShapeKind::Circle);
        let morph = trigger_morph();
        assert_eq!(*morph.start(), *ShapeKind::Square.polygon());
        assert_eq!(*morph.end(), *ShapeKind::Circle.polygon());
    }

    #[test]
    fn trigger_path_is_closed_at_every_progress_and_stays_near_its_box() {
        let rect = Rect::from_origin_size(Point::new(10.0, 20.0), Size::new(56.0, 56.0));
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let path = trigger_path(t, rect);
            assert!(
                matches!(path.elements().last(), Some(PathEl::ClosePath)),
                "t={t}"
            );
            let bounds = path.bounding_box();
            assert!(
                bounds.x0 >= rect.x0 - 1.0 && bounds.y0 >= rect.y0 - 1.0,
                "t={t} {bounds:?}"
            );
            assert!(
                bounds.x1 <= rect.x1 + 1.0 && bounds.y1 <= rect.y1 + 1.0,
                "t={t} {bounds:?}"
            );
        }
    }

    #[test]
    fn fab_shape_spring_matches_upstream_fab_shape_motion() {
        // m3e_fab_menu.dart:90-94: expressiveSpatialDefault().copyWith(damping: 0.7).
        assert_eq!(
            FAB_SHAPE_SPRING.stiffness,
            MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness
        );
        assert_eq!(FAB_SHAPE_SPRING.damping_ratio, 0.7);
        assert_eq!(FAB_SHAPE_SPRING.mass, 1.0);
    }

    #[test]
    fn trigger_container_lerps_between_the_named_endpoints() {
        assert_eq!(MAIN_CONTAINER_CLOSED, 80.0);
        assert_eq!(MAIN_CONTAINER_OPEN, 56.0);
        assert_eq!(lerp(MAIN_CONTAINER_CLOSED, MAIN_CONTAINER_OPEN, 0.0), 80.0);
        assert_eq!(lerp(MAIN_CONTAINER_CLOSED, MAIN_CONTAINER_OPEN, 1.0), 56.0);
        assert_eq!(icon_size_for(MAIN_CONTAINER_OPEN), MAIN_ICON);
        assert!(icon_size_for(MAIN_CONTAINER_CLOSED) > MAIN_ICON);
    }

    #[test]
    fn trigger_morph_is_a_layout_value_and_requests_relayout_while_in_flight() {
        let mut w = build_menu(false, 0);
        laid_out(&mut w);
        let closed_size = w.fab_rect.width();
        assert_eq!(closed_size, MAIN_CONTAINER_CLOSED, "built closed: 80dp");

        let prev = fab_menu::<Log, _>(icon_stub(), false, vec![], |_s: &mut Log| {});
        let next = fab_menu::<Log, _>(icon_stub(), true, vec![], |_s: &mut Log| {});
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));

        // The first `advance` after a fresh `fling` only seeds the clock
        // (zero delta — `AnimationController::advance`'s own documented
        // contract), so two paints at increasing frame times are needed
        // before the value actually moves — the same idiom `sheet.rs`'s own
        // `PaintCtx::for_test`-driven tests use.
        let area = Size::new(400.0, 600.0);
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, FrameTime::from_nanos(0));
        w.paint(&mut pctx, &mut rec);
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, FrameTime::from_nanos(80_000_000));
        w.paint(&mut pctx, &mut rec);
        assert!(
            pctx.needs_layout(),
            "an in-flight morph must request a relayout, not merely a repaint"
        );

        laid_out(&mut w);
        assert!(
            w.fab_rect.width() < closed_size,
            "relaying out after the spring moved shrinks the trigger toward 56dp"
        );
    }

    // -----------------------------------------------------------------------
    // Cascade stagger (acceptance: per-item delays cited).
    // -----------------------------------------------------------------------

    #[test]
    fn cascade_stagger_step_matches_upstream() {
        assert_eq!(EXPAND_STAGGER_MS, 30); // m3e_fab_menu.dart:96
    }

    #[test]
    fn item_spring_matches_expressive_spatial_press() {
        // m3e_fab_menu.dart:84-88: expressiveSpatialDefault().copyWith(damping: 0.55) ==
        // MaterialSpring::EXPRESSIVE_SPATIAL_PRESS (380, 0.55).
        assert_eq!(ITEM_SPRING.stiffness, 380.0);
        assert_eq!(ITEM_SPRING.damping_ratio, 0.55);
        assert_eq!(
            ITEM_SPRING.stiffness,
            MaterialSpring::EXPRESSIVE_SPATIAL_PRESS.stiffness
        );
        assert_eq!(
            ITEM_SPRING.damping_ratio,
            MaterialSpring::EXPRESSIVE_SPATIAL_PRESS.damping_ratio
        );
    }

    #[test]
    fn opening_cascade_fires_nearest_the_fab_first() {
        assert_eq!(cascade_delay(0, 4, true), Duration::ZERO);
        assert_eq!(cascade_delay(1, 4, true), Duration::from_millis(30));
        assert_eq!(cascade_delay(2, 4, true), Duration::from_millis(60));
        assert_eq!(cascade_delay(3, 4, true), Duration::from_millis(90));
    }

    #[test]
    fn closing_cascade_reverses_the_stagger_order() {
        assert_eq!(cascade_delay(3, 4, false), Duration::ZERO);
        assert_eq!(cascade_delay(2, 4, false), Duration::from_millis(30));
        assert_eq!(cascade_delay(1, 4, false), Duration::from_millis(60));
        assert_eq!(cascade_delay(0, 4, false), Duration::from_millis(90));
    }

    #[test]
    fn advance_cascade_launches_items_in_order_as_time_elapses() {
        let mut w = build_menu(false, 3);
        laid_out(&mut w);
        let prev = fab_menu::<Log, _>(
            icon_stub(),
            false,
            (0..3).map(item).collect(),
            |_s: &mut Log| {},
        );
        let next = fab_menu::<Log, _>(
            icon_stub(),
            true,
            (0..3).map(item).collect(),
            |_s: &mut Log| {},
        );
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.cascade_launched.iter().all(|&l| !l));

        w.advance_cascade(FrameTime::ZERO);
        assert!(
            w.cascade_launched[0],
            "index 0 (nearest the FAB) has a 0ms delay"
        );
        assert!(!w.cascade_launched[1]);
        assert!(!w.cascade_launched[2]);

        w.advance_cascade(FrameTime::from_nanos(35_000_000)); // +35ms
        assert!(w.cascade_launched[1], "30ms delay elapsed");
        assert!(!w.cascade_launched[2], "60ms delay hasn't elapsed yet");

        w.advance_cascade(FrameTime::from_nanos(65_000_000)); // +65ms total
        assert!(w.cascade_launched[2], "60ms delay elapsed");
    }

    #[test]
    fn item_count_change_rebuilds_pods_and_clears_armed() {
        let mut w = build_menu(true, 1);
        let prev = fab_menu::<Log, _>(icon_stub::<Log>(), true, vec![item(0)], |_s: &mut Log| {});
        let next = fab_menu::<Log, _>(
            icon_stub::<Log>(),
            true,
            vec![item(0), item(1), item(2)],
            |_s: &mut Log| {},
        );
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.items.len(), 3);
        assert_eq!(w.item_labels, vec!["Item 0", "Item 1", "Item 2"]);
        assert_eq!(w.item_anim.len(), 3);
        assert_eq!(
            w.cascade_launched,
            vec![true, true, true],
            "already open: settles instantly"
        );
    }

    // -----------------------------------------------------------------------
    // Controlled open + interaction (unchanged behavioral contract).
    // -----------------------------------------------------------------------

    #[test]
    fn tapping_the_closed_trigger_requests_open() {
        let mut w = build_menu(false, 2);
        laid_out(&mut w);
        let mut state = Log::default();
        let fab_center = w.fab_rect.center();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, fab_center.x, fab_center.y),
        );
        assert_eq!(w.armed, Some(Target::Fab));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, fab_center.x, fab_center.y),
        );
        assert_eq!(state.toggles, 1, "trigger tap requests an open/close flip");
        assert!(state.selected.is_empty());
    }

    #[test]
    fn tapping_the_open_trigger_again_also_requests_a_flip() {
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        let mut state = Log::default();
        let fab_center = w.fab_rect.center();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, fab_center.x, fab_center.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, fab_center.x, fab_center.y),
        );
        assert_eq!(state.toggles, 1, "tap-again closes");
    }

    #[test]
    fn item_tap_fires_its_own_callback_then_closes() {
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        assert_eq!(w.item_rects.len(), 2);
        let mut state = Log::default();
        let target = w.item_rects[0].center();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, target.x, target.y),
        );
        assert_eq!(w.armed, Some(Target::Item(0)));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, target.x, target.y),
        );
        assert_eq!(state.selected, vec![0], "the tapped item's on_select fired");
        assert_eq!(state.toggles, 1, "an item tap also requests a close");
    }

    #[test]
    fn scrim_tap_closes_without_selecting_an_item() {
        let mut w = build_menu(true, 1);
        laid_out(&mut w);
        let mut state = Log::default();
        // The top-left corner of a 400x600 area is neither the trigger
        // (bottom-right) nor any item (stacked just above the trigger).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert_eq!(w.armed, Some(Target::Scrim));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.toggles, 1, "a scrim tap requests a close");
        assert!(state.selected.is_empty());
    }

    #[test]
    fn down_outside_the_trigger_is_ignored_while_closed() {
        let mut w = build_menu(false, 1);
        laid_out(&mut w);
        let mut state = Log::default();
        let result = dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(w.armed.is_none());
    }

    #[test]
    fn cancel_disarms_without_firing() {
        let mut w = build_menu(true, 1);
        laid_out(&mut w);
        let mut state = Log::default();
        let target = w.item_rects[0].center();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, target.x, target.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Cancel, target.x, target.y),
        );
        assert!(w.armed.is_none());
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, target.x, target.y),
        );
        assert_eq!(state.toggles, 0);
        assert!(state.selected.is_empty());
    }

    #[test]
    fn release_outside_the_armed_item_fires_nothing() {
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        let mut state = Log::default();
        let item0 = w.item_rects[0].center();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, item0.x, item0.y),
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 500.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 500.0));
        assert!(state.selected.is_empty());
        assert_eq!(state.toggles, 0);
    }

    // --- Focus + Escape opt-in. ---

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn escape_while_open_requests_toggle() {
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        let mut state = Log::default();
        let result = dispatch(&mut w, &mut state, &escape_event());
        assert!(matches!(result, EventResult::Handled));
        assert_eq!(state.toggles, 1, "Escape while open requests a close");
        assert!(state.selected.is_empty());
    }

    #[test]
    fn escape_while_closed_is_ignored() {
        let mut w = build_menu(false, 2);
        laid_out(&mut w);
        let mut state = Log::default();
        let result = dispatch(&mut w, &mut state, &escape_event());
        assert!(matches!(result, EventResult::Ignored));
        assert_eq!(state.toggles, 0, "a closed menu has nothing to dismiss");
    }

    #[test]
    fn trigger_press_claims_focus_so_a_later_escape_reaches_the_open_menu() {
        // Drive a real RenderRoot so the container-level focus-path recording
        // (mirrors `cupertino::alert_dialog`'s modal-widget precedent) is
        // actually exercised, not just the widget's own `event` in isolation.
        let mut root: RenderRoot<Log, FabMenuView<Log>> = RenderRoot::new();
        let mut app = |_s: &mut Log| {
            fab_menu::<Log, _>(
                icon_stub::<Log>(),
                true,
                (0..2).map(item).collect(),
                |s: &mut Log| s.toggles += 1,
            )
        };
        let mut state = Log::default();
        root.rebuild(&mut app, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // A press on the scrim (top-left corner, outside the trigger/items —
        // see `scrim_tap_closes_without_selecting_an_item`) claims focus. A
        // `Cancel` instead of `Up` disarms without firing `on_toggle`, so the
        // Escape below is the only thing that can have requested the close.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Cancel, 5.0, 5.0));
        assert_eq!(
            state.toggles, 0,
            "the cancelled scrim press alone never toggles"
        );

        root.event(&mut state, &escape_event());
        assert_eq!(
            state.toggles, 1,
            "Escape reaches the now-focused, still-open menu and requests a close"
        );
    }

    #[test]
    fn a_down_reclaims_trigger_focus_after_an_external_blur() {
        use frust::authoring::ChildPod;

        let mut w = build_menu(false, 2);
        laid_out(&mut w);
        let fab_center = w.fab_rect.center();
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let area = Size::new(400.0, 600.0);
        // Re-run layout through the pod (idempotent — same geometry) so its
        // recorded `size` is set for `event_child`'s translation.
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut state = Log::default();
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(
                &mut ctx,
                &ev(PointerPhase::Down, fab_center.x, fab_center.y),
            );
        }
        assert!(pod.is_focused(), "the first trigger Down claims focus");
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Up, fab_center.x, fab_center.y));
        }

        // Simulate an external blur (mirrors the root's own `Down`-with-no-
        // claim release path) so the second Down's own re-claim is what's
        // under test, not a leftover flag.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(
                &mut ctx,
                &ev(PointerPhase::Down, fab_center.x, fab_center.y),
            );
        }
        assert!(
            pod.is_focused(),
            "a second trigger Down must re-claim focus after an external blur — this \
             is what keeps the root's focus/IME session alive while the menu is up"
        );
    }

    #[test]
    fn a_down_reclaims_scrim_focus_after_an_external_blur() {
        use frust::authoring::ChildPod;

        let w = build_menu(true, 2);
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let area = Size::new(400.0, 600.0);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut state = Log::default();
        // The top-left corner is neither the trigger nor any item (the
        // scrim) — mirrors `scrim_tap_closes_without_selecting_an_item`.
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(pod.is_focused(), "the first scrim Down claims focus");
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Cancel, 5.0, 5.0));
        }

        // Simulate an external blur (mirrors the root's own `Down`-with-no-
        // claim release path) so the second Down's own re-claim is what's
        // under test, not a leftover flag.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(
            pod.is_focused(),
            "a second scrim/item Down must re-claim focus after an external blur — \
             this is what keeps the root's focus/IME session alive while the menu is up"
        );
    }

    #[test]
    fn reopening_after_a_close_reclaims_focus() {
        // The persisting-widget counterpart of the pushed-page widgets' fresh
        // `build`: the FAB lives on across open/close cycles. Driven through a
        // `ChildPod` (rather than a removed internal guard field), so what's
        // actually asserted is the pod's own recorded focus path surviving a
        // close/reopen rebuild, then self-healing a later external blur.
        use frust::authoring::ChildPod;

        let mut w = build_menu(false, 1);
        laid_out(&mut w);
        let fab_center = w.fab_rect.center();
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let area = Size::new(400.0, 600.0);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut state = Log::default();
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(
                &mut ctx,
                &ev(PointerPhase::Down, fab_center.x, fab_center.y),
            );
        }
        assert!(pod.is_focused(), "the trigger press claims focus");

        // Open, then close again via rebuild (the controlled `open` prop),
        // driven directly on the pod's boxed widget.
        let mut counter = 0u64;
        let closed = fab_menu::<Log, _>(icon_stub::<Log>(), false, vec![item(0)], |s: &mut Log| {
            s.toggles += 1
        });
        let opened = fab_menu::<Log, _>(icon_stub::<Log>(), true, vec![item(0)], |s: &mut Log| {
            s.toggles += 1
        });
        {
            let widget = pod
                .widget_mut()
                .downcast_mut::<FabMenuWidget>()
                .expect("the pod wraps a FabMenuWidget");
            View::<Log>::rebuild(&opened, &closed, widget, &mut BuildCtx::new(&mut counter));
            View::<Log>::rebuild(&closed, &opened, widget, &mut BuildCtx::new(&mut counter));
        }
        assert!(
            pod.is_focused(),
            "a same-identity rebuild (open then close) never touches the pod's \
             recorded focus path on its own"
        );

        // Simulate an external blur (mirrors the root's own `Down`-with-no-
        // claim release path) so the reopened trigger's own re-claim is
        // what's under test.
        pod.set_focused(false);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));
        {
            let s: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(
                &mut ctx,
                &ev(PointerPhase::Down, fab_center.x, fab_center.y),
            );
        }
        assert!(
            pod.is_focused(),
            "reopening after a close still lets a fresh trigger press re-claim focus"
        );
    }

    #[test]
    fn is_controlled_open_only_moves_via_rebuild() {
        let mut w = build_menu(false, 1);
        assert!(!w.open);
        let prev = fab_menu::<Log, _>(icon_stub::<Log>(), false, vec![item(0)], |_s: &mut Log| {});
        let next = fab_menu::<Log, _>(icon_stub::<Log>(), true, vec![item(0)], |_s: &mut Log| {});
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.open, "open state moved on rebuild (controlled)");
        assert!(
            w.fab_morph.is_animating(),
            "the trigger morph relaunched toward the open target"
        );
    }

    // -----------------------------------------------------------------------
    // Paint.
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct RectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: Vec<(Point, Size, Color)>,
        fills: Vec<BezPath>,
        /// One entry per `push_layer` — the item-fade contract (see the
        /// module docs' Cascade stagger section).
        layers: Vec<(Point, Size, f32)>,
        pops: usize,
    }
    impl PaintScene for RectRecorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_path(&mut self, _origin: Point, path: &BezPath, _brush: &Brush) {
            self.fills.push(path.clone());
        }
        fn push_layer(&mut self, o: Point, s: Size, alpha: f32) {
            self.layers.push((o, s, alpha));
        }
        fn pop_layer(&mut self) {
            self.pops += 1;
        }
    }

    /// Paint one frame at `millis` into `rec` (the cascade's own clock is
    /// seeded from the first such paint — see [`FabMenuWidget::advance_cascade`]).
    fn paint_at(w: &mut FabMenuWidget, millis: u64, rec: &mut RectRecorder) {
        let mut pctx = PaintCtx::for_test(
            Point::ZERO,
            Size::new(400.0, 600.0),
            FrameTime::from_nanos(millis * 1_000_000),
        );
        w.paint(&mut pctx, rec);
    }

    /// Flip the controlled `open` prop through a rebuild — the only way it
    /// moves (see the module docs).
    fn set_open(w: &mut FabMenuWidget, from: bool, to: bool, item_count: usize) {
        let prev = fab_menu::<Log, _>(
            icon_stub(),
            from,
            (0..item_count).map(item).collect(),
            |_s: &mut Log| {},
        );
        let next = fab_menu::<Log, _>(
            icon_stub(),
            to,
            (0..item_count).map(item).collect(),
            |_s: &mut Log| {},
        );
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, w, &mut BuildCtx::new(&mut counter));
    }

    #[test]
    fn closed_paint_draws_only_the_trigger_shape() {
        let mut w = build_menu(false, 2);
        laid_out(&mut w);
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0));
        w.paint(&mut pctx, &mut rec);
        assert!(rec.rects.is_empty(), "no scrim while closed");
        assert!(rec.rrects.is_empty(), "no item containers while closed");
        assert!(rec.layers.is_empty(), "no item fade layers while closed");
        assert_eq!(rec.fills.len(), 1, "only the trigger's morphed outline");
    }

    #[test]
    fn open_paint_draws_scrim_and_item_containers() {
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        open_fully(&mut w);
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0));
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rects.len(), 1, "one full-area scrim fill");
        assert_eq!(rec.rrects.len(), 2, "2 item containers");
        assert_eq!(rec.fills.len(), 1, "the trigger's morphed outline");
        // Fully-revealed items still composite through their own layer (one
        // per visible item, at their own progress) — the container itself
        // carries the role color unmodified, never an alpha-scaled copy.
        assert_eq!(rec.layers.len(), 2, "one fade layer per visible item");
        assert_eq!(rec.pops, 2, "every pushed layer is popped again");
        assert!(
            rec.layers.iter().all(|l| l.2 == 1.0),
            "settled open: t == 1"
        );
        assert_eq!(rec.rrects[0].3, PRIMARY_CONTAINER);
        assert_eq!(rec.rrects[1].3, SECONDARY_CONTAINER);
        // Upstream's own scrimOpacity default is 0.0 — see the module docs'
        // Scrim section.
        assert_eq!(rec.rects[0].2, with_alpha(SCRIM_FALLBACK, 0.0));
    }

    #[test]
    fn themed_paint_resolves_container_family_tokens() {
        let theme = crate::baseline();
        let mut w = build_menu(true, 3);
        laid_out(&mut w);
        open_fully(&mut w);
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.primary_container);
        assert_eq!(rec.rrects[1].3, scheme.secondary_container);
        assert_eq!(rec.rrects[2].3, scheme.tertiary_container);
    }

    #[test]
    fn item_shape_is_a_full_pill() {
        assert_eq!(item_radius(), ITEM_HEIGHT / 2.0);
    }

    // -----------------------------------------------------------------------
    // Reveal-opacity contract: no bare-content flash, in either direction.
    // -----------------------------------------------------------------------

    #[test]
    fn a_mid_cascade_item_fades_as_one_unit_through_its_own_layer() {
        let mut w = build_menu(false, 2);
        laid_out(&mut w);
        set_open(&mut w, false, true, 2);

        let mut rec = RectRecorder::default();
        // The first paint only seeds the cascade clock and flings item 0
        // (`advance`'s zero-delta contract); the second lands it mid-reveal,
        // while item 1's own 30ms delay hasn't elapsed.
        paint_at(&mut w, 0, &mut rec);
        rec = RectRecorder::default();
        paint_at(&mut w, 20, &mut rec);

        let t = w.item_anim[0].value_clamped();
        assert!(t > 0.0 && t < 1.0, "item 0 is mid-reveal (t={t})");
        assert_eq!(
            w.item_anim[1].value_clamped(),
            0.0,
            "item 1 hasn't launched"
        );

        assert_eq!(rec.layers.len(), 1, "one fade layer per *visible* item");
        assert_eq!(rec.pops, 1, "and it is popped again");
        assert_eq!(
            rec.layers[0].2, t as f32,
            "the layer carries the item's own reveal progress"
        );
        let item = w.item_rects[0];
        assert_eq!(rec.layers[0].0, item.origin(), "layer covers the item row");
        assert_eq!(rec.layers[0].1, item.size());
        assert_eq!(rec.rrects.len(), 1, "only the revealed item's container");
        assert_eq!(
            rec.rrects[0].3, PRIMARY_CONTAINER,
            "the container paints at full alpha INSIDE the layer — fading it \
             again there would double-fade it, and fading it alone (while the \
             icon/label pods paint opaque regardless) is what made a low-`t` \
             item read as a bare content row with no pill behind it"
        );
    }

    #[test]
    fn a_menu_built_closed_never_flings_and_never_paints_an_item() {
        let mut w = build_menu(false, 3);
        laid_out(&mut w);
        assert_eq!(
            w.cascade_launched,
            vec![true; 3],
            "a widget built at rest mounts fully-launched: an unseeded closed \
             mount lets the first paint fling every item from rest with the \
             retreat velocity, dipping below zero and rebounding back above it"
        );
        assert!(
            w.item_anim
                .iter()
                .all(|a| !a.is_animating() && a.value() == 0.0),
            "every item spring rests exactly at 0"
        );
        assert!(!w.fab_morph.is_animating() && w.fab_morph.value() == 0.0);

        let mut rec = RectRecorder::default();
        for frame in 0..40u64 {
            paint_at(&mut w, frame * 16, &mut rec);
        }
        assert!(rec.rrects.is_empty(), "no item container ever paints");
        assert!(rec.layers.is_empty(), "no item fade layer ever pushes");
        assert!(
            w.item_anim.iter().all(|a| !a.is_animating()),
            "and no item spring ever launched"
        );
    }

    #[test]
    fn the_closing_retreat_stops_at_zero_instead_of_ringing_back_above_it() {
        let mut w = build_menu(true, 1);
        laid_out(&mut w);
        open_fully(&mut w);
        set_open(&mut w, true, false, 1);

        // ITEM_SPRING is under-damped (0.55): released from 1.0 at
        // -FLING_VELOCITY it first reaches zero around 124ms, then — unclamped
        // — rings back *above* zero twice before its energy dies.
        let mut rec = RectRecorder::default();
        for frame in 0..13u64 {
            paint_at(&mut w, frame * 16, &mut rec); // through 192ms
        }
        assert!(
            !w.item_anim[0].is_animating(),
            "the retreating spring stops the frame it reaches zero"
        );
        assert_eq!(w.item_anim[0].value(), 0.0, "and stops exactly *at* zero");

        let mut after = RectRecorder::default();
        for frame in 13..60u64 {
            paint_at(&mut w, frame * 16, &mut after); // through ~950ms
        }
        assert!(
            after.rrects.is_empty() && after.layers.is_empty(),
            "no rebound ever paints the item back on"
        );
    }

    #[test]
    fn the_trigger_morphs_retreat_rebound_is_imperceptible_so_it_stays_unclamped() {
        // The trigger morph rings around its 0.0 target exactly like an item's
        // spring does, but it drives a *layout* size rather than an opacity and
        // FAB_SHAPE_SPRING is damped at 0.7 rather than 0.55. Measured here
        // rather than assumed: the first rebound above zero peaks near
        // `+0.0021`, worth ~0.05px of container size, so there is nothing to
        // clamp away. This pins the measurement the no-clamp decision rests on
        // — re-take it if either spring constant moves.
        let mut anim = AnimationController::new(ANIM_PERIOD);
        snap_anim(&mut anim, true, FrameTime::ZERO);
        anim.fling(-FLING_VELOCITY, FAB_SHAPE_SPRING);
        let (mut crossed, mut peak) = (false, 0.0f64);
        for frame in 0..1000u64 {
            anim.advance(FrameTime::from_nanos(frame * 1_000_000)); // 1ms steps
            let v = anim.value();
            crossed |= v <= 0.0;
            if crossed {
                peak = peak.max(v);
            }
        }
        assert!(crossed, "the retreat does cross its target");
        let px = MAIN_CONTAINER_CLOSED - lerp(MAIN_CONTAINER_CLOSED, MAIN_CONTAINER_OPEN, peak);
        assert!(
            px < 0.1,
            "rebound peak {peak} = {px}px of container size — big enough to see, \
             so the retreat clamp now needs to cover the morph too"
        );
    }

    #[test]
    fn reduce_motion_snaps_the_trigger_and_items_without_requesting_a_frame() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(w.fab_morph.value_clamped(), 1.0, "snapped straight open");
        assert!(
            w.item_anim.iter().all(|a| a.value_clamped() == 1.0),
            "every item snapped straight open"
        );
        assert!(
            !pctx.needs_frame(),
            "reduce_motion must not request a continuation frame"
        );
    }

    #[test]
    fn inspect_surfaces_icon_and_label_pods_per_item_plus_the_trigger() {
        // Regression coverage for the hand-written `FabMenuItemPod`
        // `VisitPods` impl (ported from
        // crates/frust-widgets/tests/visit_children.rs, which covered this
        // shape before the design-system extraction). If
        // `FabMenuItemPod::visit_pods` ever stopped forwarding one of
        // `icon`/`label`, the child count and TextWidget count below would
        // both fall short of 5 — a dropped-child regression
        // `RenderRoot::inspect()`/devtools depend on this seam to never
        // allow.
        fn logic(_s: &mut ()) -> FabMenuView<()> {
            fab_menu::<(), _>(
                icon_stub::<()>(),
                true,
                vec![
                    fab_menu_item(icon_stub::<()>(), "alpha", |_s: &mut ()| {}),
                    fab_menu_item(icon_stub::<()>(), "beta", |_s: &mut ()| {}),
                ],
                |_s: &mut ()| {},
            )
        }
        let mut root: RenderRoot<(), FabMenuView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let nodes = root.inspect();

        let short_names: Vec<&str> = nodes
            .iter()
            .map(|n| {
                let bare = n.type_name.split('<').next().unwrap_or(n.type_name);
                bare.rsplit("::").next().unwrap_or(bare)
            })
            .collect();
        assert_eq!(
            short_names
                .iter()
                .filter(|n| **n == "FabMenuWidget")
                .count(),
            1
        );
        assert_eq!(
            nodes[0].children.len(),
            5,
            "two items x (icon + label), plus the trigger icon"
        );
        assert_eq!(
            short_names.iter().filter(|n| **n == "TextWidget").count(),
            5
        );
    }

    #[test]
    fn semantics_reports_closed_trigger_then_open_menu() {
        fn logic_closed(_s: &mut ()) -> FabMenuView<()> {
            fab_menu::<(), _>(leaf_any(24.0, 24.0), false, vec![], |_s: &mut ()| {})
                .label("Compose")
        }
        let mut root: frust_core::RenderRoot<(), FabMenuView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic_closed, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("closed menu contributes a Role::Button trigger node");
        assert_eq!(node.label(), Some("Compose"));
        assert_eq!(node.is_expanded(), Some(false));

        fn logic_open(_s: &mut ()) -> FabMenuView<()> {
            fab_menu::<(), _>(
                leaf_any(24.0, 24.0),
                true,
                vec![fab_menu_item(
                    leaf_any(24.0, 24.0),
                    "New note",
                    |_s: &mut ()| {},
                )],
                |_s: &mut ()| {},
            )
        }
        let mut root: frust_core::RenderRoot<(), FabMenuView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic_open, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let menu = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Menu)
            .expect("an open menu contributes a Role::Menu container node");
        assert_eq!(menu.1.children().len(), 1);
        let menu_item = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::MenuItem)
            .expect("an item contributes a Role::MenuItem node");
        assert_eq!(menu_item.1.label(), Some("New note"));
    }
}
