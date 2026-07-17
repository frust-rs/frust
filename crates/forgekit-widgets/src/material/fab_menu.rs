//! The Material 3 Expressive **FAB menu** (Phase 6f, PLAN.md Phase B item 5,
//! task 08): a trigger FAB that reveals a vertical stack of large menu items
//! on tap, replacing the older speed-dial pattern (RESEARCH.md:78;
//! m3.material.io/blog/building-with-m3-expressive; Compose
//! `FloatingActionButtonMenu`).
//!
//! [`fab_menu`] takes the trigger's icon, the controlled `open` flag, and a
//! list of [`FabMenuItem`]s (built with [`fab_menu_item`]: icon + label +
//! `on_select`). It is a **controlled component** (see `docs/CODE_STANDARDS.md`'s
//! Interaction Semantics), mirroring [`super::split_button`]'s `open` prop: the
//! widget never flips `open` itself. A single `on_toggle` callback fires on
//! every gesture that requests an open/close flip — a tap on the closed
//! trigger, a tap on the open trigger ("tap-again"), a completed tap on a menu
//! item (after that item's own `on_select` runs), and a tap on the scrim
//! (anywhere else in the widget's area while open) — and the app is expected
//! to toggle its own `open` state in that handler, exactly like
//! [`super::split_button`]'s `on_open` convention.
//!
//! # Reveal geometry
//!
//! The trigger sits [`EDGE_MARGIN`] from the widget's own bottom-right corner
//! (this widget fills its box constraints, like [`super::sheet`], so it is
//! meant to be the top layer of a full-area [`crate::Stack`]). Items stack
//! upward from the trigger, right-aligned to its trailing edge, closest item
//! first (index `0` sits nearest the trigger). Reuses [`super::fab`]'s
//! regular-tier tokens for the trigger (56dp container, 24dp icon,
//! `shape.large` corner radius, `primary_container`/level-3 shadow) rather
//! than importing them (they are private to that module) — see the constants
//! below, each documented against its `fab.rs` counterpart.
//!
//! # Item colors ("contrasting… primary-container family")
//!
//! Each item's container cycles through the three M3 "container" roles —
//! `primary_container`, `secondary_container`, `tertiary_container` (by
//! `index % 3`) — so adjacent items read as visually distinct without a
//! per-item color prop. Item labels use [`ThemeTextColor::OnSurface`]
//! uniformly regardless of which container family the item sits on, the same
//! choice [`super::button_group`] documents (selection/emphasis is conveyed by
//! the container fill, not the label color, to avoid adding a new
//! `on_*_container` text role to `text.rs` — a file this task does not own).
//!
//! # Open/close motion (v1: fade, not slide)
//!
//! Open/close is spring-animated: [`REVEAL_SPRING`] is the exact
//! `MotionScheme::m3_expressive().default_spatial` preset (stiffness 700, ζ
//! 0.9), the same token [`super::split_button`]'s chevron uses — a named
//! constant, not a paint-time theme read, for the same reason (the app confirms
//! `open` on rebuild, which threads no theme; `reveal_spring_matches_motion_scheme`
//! is the tripwire). **v1 simplification**: the spring drives each revealed
//! item's container *opacity* (and the scrim's), not a position slide — the
//! item/icon/label geometry is always laid out at its rest position, and
//! `PaintScene` has no generic layer-opacity primitive to fade an arbitrary
//! child subtree, so the icon/label pop in at full opacity once the spring
//! value is non-zero while only the container rect fades. A future task could
//! add a translate-in slide once per-child paint offsetting has a supported
//! pattern (no consumer in this crate does that yet).
//!
//! # Hit-testing while closed
//!
//! While `open` is `false`, only the trigger's own rect is interactive — a
//! `Down` anywhere else in the widget's (full-area) bounds returns
//! [`EventResult::Ignored`], which `Stack::event`'s reverse-order routing
//! (`crate::route_event`) then hands to whatever sits below in the Z-order.
//! Once `open` is `true`, every other point in the area is treated as the
//! scrim (dismiss-on-tap) — see [`Target::Scrim`].
//!
//! # No `StateLayer` (v1)
//!
//! Unlike [`super::fab`], this widget paints no hover/press `StateLayer`
//! overlay on the trigger or the items — a future task can add one mirroring
//! `fab.rs`'s.
//!
//! # Keyboard operability (task 6f-fix-1/02)
//!
//! **Escape-to-dismiss now works, once the menu has focus.** A `Down` on the
//! trigger, or (while `open`) on an item or the scrim, claims focus via
//! `EventCtx::request_focus` — the same opt-in
//! [`crate::material::dialog`]/[`crate::material::sheet`]/
//! [`crate::cupertino::alert_dialog`]/[`crate::cupertino::action_sheet`] (the
//! four modal widgets `docs/ARCHITECTURE.md`'s Semantics section names) use;
//! this widget now joins that set. Once focused, a focus-routed
//! `Key(Escape)` fires the same `on_toggle` callback a scrim/item/trigger tap
//! would, but only while `open` — a closed menu has nothing to dismiss, so
//! Escape is ignored outright regardless of focus. As with the other four
//! modal widgets, there is still no hook to auto-focus the menu on open; a
//! caller must complete one pointer interaction with the open menu before
//! Escape does anything.

use std::rc::Rc;
use std::time::Duration;

use forgekit_core::accesskit::{Action, Role};
use forgekit_core::{
    AnimationController, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx,
    EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase,
    SemanticsCtx, SpringDesc, View, Widget,
};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, Size};
use peniko::Color;

use crate::text::{self, ThemeTextColor};

/// Container size of the trigger FAB, in logical px — matches [`super::fab`]'s
/// regular-tier `REGULAR_CONTAINER` (androidx `FabBaselineTokens.ContainerWidth`).
const MAIN_CONTAINER: f64 = 56.0;
/// Icon size inside the trigger FAB, in logical px — matches [`super::fab`]'s
/// `REGULAR_ICON`.
const MAIN_ICON: f64 = 24.0;
/// Unthemed-fallback corner radius for the trigger FAB (a theme resolves
/// `shape.large`) — matches [`super::fab`]'s `REGULAR_RADIUS`.
const MAIN_RADIUS: f64 = 16.0;

/// Distance from the host area's right/bottom edges to the trigger FAB, in
/// logical px.
///
/// **Community-approximate**: a FAB's screen-edge margin is a common M3
/// layout convention (16dp), not an independently-cited research-ledger value
/// for this task.
const EDGE_MARGIN: f64 = 16.0;

/// Fixed height of a menu-item row, in logical px — mirrors [`super::fab`]'s
/// extended-FAB row height (`EXTENDED_HEIGHT`), which the M3 FAB-menu item
/// pill visually resembles.
const ITEM_HEIGHT: f64 = 56.0;
/// Icon size inside a menu-item row, in logical px.
const ITEM_ICON: f64 = 24.0;
/// Horizontal padding inside a menu-item row, in logical px — **v1
/// approximation**, mirroring [`super::fab`]'s extended-FAB padding constants
/// (see that module's docs).
const ITEM_PAD_X: f64 = 16.0;
/// Gap between a menu-item's icon and its label, in logical px — **v1
/// approximation**, mirroring [`super::fab`]'s `EXTENDED_GAP`.
const ITEM_ICON_LABEL_GAP: f64 = 8.0;
/// Vertical gap between two stacked menu items, in logical px — **v1
/// approximation**.
const ITEM_GAP: f64 = 12.0;
/// Vertical gap between the trigger FAB and the nearest (index `0`) menu item,
/// in logical px — **v1 approximation**.
const FAB_TO_ITEMS_GAP: f64 = 16.0;
/// Unthemed-fallback menu-item corner radius (a theme resolves `shape.large`,
/// the same token the trigger FAB uses).
const ITEM_RADIUS: f64 = 16.0;

/// Unthemed-fallback trigger-FAB container fill (a theme resolves
/// `colors.primary_container`) — matches [`super::fab`]'s `CONTAINER`.
const MAIN_CONTAINER_COLOR: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);

/// Unthemed-fallback item container fill, `index % 3 == 0` family (a theme
/// resolves `colors.primary_container`) — matches
/// [`forgekit_theme::color::ColorScheme::m3_baseline`]'s light default.
const PRIMARY_CONTAINER: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);
/// Unthemed-fallback item container fill, `index % 3 == 1` family (a theme
/// resolves `colors.secondary_container`).
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback item container fill, `index % 3 == 2` family (a theme
/// resolves `colors.tertiary_container`).
const TERTIARY_CONTAINER: Color = Color::from_rgb8(0xFF, 0xD8, 0xE4);

/// Scrim opacity behind the open menu (M3 spec: 32%, matching
/// [`super::sheet`]/[`super::dialog`]'s scrim), scaled by the reveal spring's
/// current value so it fades in/out with the items.
const SCRIM_ALPHA: f32 = 0.32;
/// Unthemed-fallback scrim base color (a theme resolves `colors.scrim`).
const SCRIM_FALLBACK: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Unthemed-fallback shadow y-offset for the trigger FAB, matching
/// `Elevation::m3().level3`'s `y_offset` exactly — matches [`super::fab`]'s
/// `FALLBACK_SHADOW_Y_OFFSET`.
const FALLBACK_SHADOW_Y_OFFSET: f64 = 4.0;
/// Unthemed-fallback shadow blur std-dev — matches [`super::fab`]'s
/// `FALLBACK_SHADOW_BLUR`.
const FALLBACK_SHADOW_BLUR: f64 = 6.0;
/// Unthemed-fallback shadow color — matches [`super::fab`]'s
/// `FALLBACK_SHADOW_COLOR`.
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The reveal open/close spring: the exact
/// `MotionScheme::m3_expressive().default_spatial` preset (stiffness 700, ζ
/// 0.9, mass 1) — the same token [`super::split_button`]'s chevron uses. See
/// the [module docs](self) for why it is a constant.
const REVEAL_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 700.0,
    damping_ratio: 0.9,
};

/// Nominal period seeding the reveal [`AnimationController`]'s clock; the
/// motion is spring-driven ([`REVEAL_SPRING`]) via `fling`, so this only backs
/// the controller's construction.
const REVEAL_ANIM_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (value-units/sec) for the reveal open/close `fling`.
const FLING_VELOCITY: f64 = 3.0;

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Coerce a possibly-infinite constraint dimension to a finite value (this
/// widget fills its area like [`super::sheet`] — a navigator page or a
/// full-screen [`crate::Stack`] gives it bounded constraints).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The trigger-FAB container fill. Themed: `colors.primary_container`.
/// Unthemed: [`MAIN_CONTAINER_COLOR`].
fn resolve_main_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().primary_container,
        None => MAIN_CONTAINER_COLOR,
    }
}

/// The trigger-FAB corner radius. Themed: `shape.large`. Unthemed:
/// [`MAIN_RADIUS`].
fn resolve_main_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.large,
        None => MAIN_RADIUS,
    }
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters at M3
/// elevation level 3 — identical resolution to [`super::fab`]'s.
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = theme.elevation.level3;
            let color = with_alpha(theme.scheme().shadow, level.shadow.color_alpha);
            (level.shadow.blur_std_dev, level.shadow.y_offset, color)
        }
        None => (
            FALLBACK_SHADOW_BLUR,
            FALLBACK_SHADOW_Y_OFFSET,
            FALLBACK_SHADOW_COLOR,
        ),
    }
}

/// The resolved scrim fill (base color at [`SCRIM_ALPHA`], further scaled by
/// the caller's reveal progress). Themed: `colors.scrim`.
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

/// The item corner radius. Themed: `shape.large`. Unthemed: [`ITEM_RADIUS`].
fn resolve_item_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.large,
        None => ITEM_RADIUS,
    }
}

/// Build a menu item's visible label view, themed [`ThemeTextColor::OnSurface`]
/// — see the [module docs](self) for why every item uses this role uniformly.
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    forgekit_core::any::<State, _>(text::text(label).themed_role(ThemeTextColor::OnSurface))
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
    on_select: Rc<dyn Fn(&mut State)>,
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
        on_select: Rc::new(on_select),
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
    on_toggle: Rc<dyn Fn(&mut State)>,
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
        on_toggle: Rc::new(on_toggle),
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
    on_select: crate::ErasedCallback,
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
    /// The reveal open/close spring (0 = closed, 1 = open).
    reveal_anim: AnimationController,
    /// The trigger's rect, in local space, from [`Widget::layout`].
    fab_rect: Rect,
    /// Each item's rect, in local space, index `0` nearest the trigger.
    item_rects: Vec<Rect>,
    /// The armed target of an in-flight press (`None` when idle).
    armed: Option<Target>,
    /// Whether the armed pointer is currently inside the armed target's rect
    /// (always `true` for [`Target::Scrim`] — see [`Widget::event`]).
    pressed_inside: bool,
    on_toggle: crate::ErasedCallback,
}

impl FabMenuWidget {
    /// (Re)launch the reveal spring toward the current `open` target.
    fn drive_reveal(&mut self) {
        let velocity = if self.open {
            FLING_VELOCITY
        } else {
            -FLING_VELOCITY
        };
        self.reveal_anim.fling(velocity, REVEAL_SPRING);
    }
}

impl<State: 'static> View<State> for FabMenuView<State> {
    type Element = FabMenuWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FabMenuWidget {
        let items = self
            .items
            .iter()
            .map(|item| FabMenuItemPod {
                icon: crate::build_child(&item.icon, ctx),
                label: crate::build_child(&label_view::<State>(item.label.clone()), ctx),
                on_select: crate::erase_callback(&item.on_select),
            })
            .collect();
        let mut reveal_anim = AnimationController::new(REVEAL_ANIM_PERIOD);
        // Seed the resting value so a menu built already-open shows its items
        // without needing a frame to spring into them.
        if self.open {
            reveal_anim.fling(FLING_VELOCITY, REVEAL_SPRING);
        }
        FabMenuWidget {
            icon: crate::build_child(&self.icon, ctx),
            items,
            item_labels: self.items.iter().map(|i| i.label.clone()).collect(),
            open: self.open,
            label: self.label.clone(),
            reveal_anim,
            fab_rect: Rect::ZERO,
            item_rects: Vec::new(),
            armed: None,
            pressed_inside: false,
            on_toggle: crate::erase_callback(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FabMenuWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = crate::erase_callback(&self.on_toggle);
        let mut flags = ChangeFlags::NONE;

        flags |= crate::rebuild_child(&prev.icon, &self.icon, &mut element.icon, ctx);

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
                crate::teardown_child(&old_item.icon, &mut pod.icon, ctx);
                let old_label_view = label_view::<State>(old_item.label.clone());
                crate::teardown_child(&old_label_view, &mut pod.label, ctx);
            }
            element.items = self
                .items
                .iter()
                .map(|item| FabMenuItemPod {
                    icon: crate::build_child(&item.icon, ctx),
                    label: crate::build_child(&label_view::<State>(item.label.clone()), ctx),
                    on_select: crate::erase_callback(&item.on_select),
                })
                .collect();
            element.item_labels = self.items.iter().map(|i| i.label.clone()).collect();
            element.armed = None;
            element.pressed_inside = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (i, (prev_item, next_item)) in prev.items.iter().zip(self.items.iter()).enumerate()
            {
                let pod = &mut element.items[i];
                flags |= crate::rebuild_child(&prev_item.icon, &next_item.icon, &mut pod.icon, ctx);
                if prev_item.label != next_item.label {
                    let prev_view = label_view::<State>(prev_item.label.clone());
                    let next_view = label_view::<State>(next_item.label.clone());
                    flags |= crate::rebuild_child(&prev_view, &next_view, &mut pod.label, ctx);
                    element.item_labels[i] = next_item.label.clone();
                }
                pod.on_select = crate::erase_callback(&next_item.on_select);
            }
        }

        flags
    }

    fn teardown(&self, element: &mut FabMenuWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.icon, &mut element.icon, ctx);
        for (item, pod) in self.items.iter().zip(element.items.iter_mut()) {
            crate::teardown_child(&item.icon, &mut pod.icon, ctx);
            let label_view = label_view::<State>(item.label.clone());
            crate::teardown_child(&label_view, &mut pod.label, ctx);
        }
    }
}

impl Widget for FabMenuWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        // Trigger FAB: bottom-right anchored.
        let icon_size = self
            .icon
            .layout_child(ctx, &BoxConstraints::tight(Size::new(MAIN_ICON, MAIN_ICON)));
        let fab_origin = Point::new(
            (area_w - EDGE_MARGIN - MAIN_CONTAINER).max(0.0),
            (area_h - EDGE_MARGIN - MAIN_CONTAINER).max(0.0),
        );
        self.fab_rect =
            Rect::from_origin_size(fab_origin, Size::new(MAIN_CONTAINER, MAIN_CONTAINER));
        self.icon.set_origin(Point::new(
            fab_origin.x + (MAIN_CONTAINER - icon_size.width) / 2.0,
            fab_origin.y + (MAIN_CONTAINER - icon_size.height) / 2.0,
        ));

        // Items stack upward from the trigger, right-aligned to its trailing
        // edge, index 0 nearest the trigger.
        self.item_rects.clear();
        let right_edge = self.fab_rect.x1;
        let mut bottom = self.fab_rect.y0 - FAB_TO_ITEMS_GAP;
        for pod in self.items.iter_mut() {
            let icon_size = pod
                .icon
                .layout_child(ctx, &BoxConstraints::tight(Size::new(ITEM_ICON, ITEM_ICON)));
            let label_size = pod.label.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, ITEM_HEIGHT)),
            );
            let content_w = icon_size.width + ITEM_ICON_LABEL_GAP + label_size.width;
            let item_w = content_w + ITEM_PAD_X * 2.0;
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
        // `pod.*.paint_child(ctx, ...)` calls below.
        let theme = Theme::from_paint_ctx(ctx);
        let scrim_base = resolve_scrim_base(theme);
        let item_radius = resolve_item_radius(theme);
        let item_colors: Vec<Color> = (0..self.items.len())
            .map(|i| resolve_item_color(theme, i))
            .collect();
        let main_color = resolve_main_color(theme);
        let main_radius = resolve_main_radius(theme);
        let (blur, y_offset, shadow_color) = resolve_shadow(theme);

        let origin = ctx.origin();

        let animating = self.reveal_anim.advance(ctx.frame_time());
        let t = self.reveal_anim.value_clamped();

        if t > 0.0 {
            let scrim = with_alpha(scrim_base, SCRIM_ALPHA * t as f32);
            scene.fill_rect(origin, ctx.size(), scrim);

            for (i, (pod, rect)) in self
                .items
                .iter_mut()
                .zip(self.item_rects.iter())
                .enumerate()
            {
                let container = with_alpha(item_colors[i], t as f32);
                scene.fill_rounded_rect(
                    Point::new(origin.x + rect.x0, origin.y + rect.y0),
                    rect.size(),
                    item_radius,
                    container,
                );
                pod.icon.paint_child(ctx, scene);
                pod.label.paint_child(ctx, scene);
            }
        }

        // Trigger FAB: always painted, on top of the scrim/items.
        scene.draw_shadow(
            Point::new(
                origin.x + self.fab_rect.x0,
                origin.y + self.fab_rect.y0 + y_offset,
            ),
            self.fab_rect.size(),
            main_radius,
            blur,
            shadow_color,
        );
        scene.fill_rounded_rect(
            Point::new(origin.x + self.fab_rect.x0, origin.y + self.fab_rect.y0),
            self.fab_rect.size(),
            main_radius,
            main_color,
        );
        self.icon.paint_child(ctx, scene);

        if animating {
            ctx.request_frame();
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
                    // A press on the trigger claims focus, so a subsequent
                    // Escape has a focus chain to travel (see the module
                    // docs' Keyboard operability note).
                    ctx.request_focus();
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
                // also claims focus.
                ctx.request_focus();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf_any;
    use forgekit_core::{KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot};
    use forgekit_theme::MotionScheme;
    use std::any::Any;

    /// A minimal `State`-generic icon stand-in (mirrors `fab.rs`'s
    /// `icon_stub`: `test_support::leaf_any` only implements `View<()>`, but
    /// this module's tests need a generic `State` — a plain themed
    /// [`crate::text::text`] run stands in for the icon, its content
    /// irrelevant to firing behavior).
    fn icon_stub<State: 'static>() -> AnyView<State> {
        forgekit_core::any::<State, _>(text::text("i"))
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
        let mut tcx = forgekit_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));
    }

    #[test]
    fn reveal_spring_matches_motion_scheme() {
        let default_spatial = MotionScheme::m3_expressive().default_spatial;
        assert_eq!(REVEAL_SPRING.stiffness, default_spatial.stiffness);
        assert_eq!(REVEAL_SPRING.damping_ratio, default_spatial.damping_ratio);
        assert_eq!(REVEAL_SPRING.mass, 1.0);
    }

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

    // --- Focus + Escape opt-in (task 6f-fix-1/02). ---

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
        let mut tcx = forgekit_text::TextContext::new();
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
    fn is_controlled_open_only_moves_via_rebuild() {
        let mut w = build_menu(false, 1);
        assert!(!w.open);
        let prev = fab_menu::<Log, _>(icon_stub::<Log>(), false, vec![item(0)], |_s: &mut Log| {});
        let next = fab_menu::<Log, _>(icon_stub::<Log>(), true, vec![item(0)], |_s: &mut Log| {});
        let mut counter = 0u64;
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.open, "open state moved on rebuild (controlled)");
        assert!(
            w.reveal_anim.is_animating(),
            "the reveal spring relaunched toward the open target"
        );
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
    }

    #[derive(Default)]
    struct RectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: Vec<(Point, Size, Color)>,
    }
    impl PaintScene for RectRecorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    #[test]
    fn closed_paint_draws_only_the_trigger() {
        let mut w = build_menu(false, 2);
        laid_out(&mut w);
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0));
        w.paint(&mut pctx, &mut rec);
        assert!(rec.rects.is_empty(), "no scrim while closed");
        assert_eq!(rec.rrects.len(), 1, "only the trigger fill");
        assert_eq!(rec.rrects[0].3, MAIN_CONTAINER_COLOR);
    }

    #[test]
    fn open_paint_draws_scrim_and_item_containers() {
        let mut w = build_menu(true, 2);
        laid_out(&mut w);
        // Seed then advance the reveal spring so it has settled at a non-zero
        // (fully open) value before painting — mirrors
        // `button_group`'s `pressing_adds_a_morphing_emphasis_path`.
        w.reveal_anim.advance(forgekit_core::FrameTime::ZERO);
        w.reveal_anim
            .advance(forgekit_core::FrameTime::from_nanos(500_000_000));
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0));
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rects.len(), 1, "one full-area scrim fill");
        // 2 item containers + 1 trigger = 3 rounded rects.
        assert_eq!(rec.rrects.len(), 3);
        assert_eq!(rec.rrects[0].3, with_alpha(PRIMARY_CONTAINER, 1.0));
        assert_eq!(rec.rrects[1].3, with_alpha(SECONDARY_CONTAINER, 1.0));
    }

    #[test]
    fn themed_paint_resolves_container_family_tokens() {
        let theme = Theme::m3_baseline();
        let mut w = build_menu(true, 3);
        laid_out(&mut w);
        w.reveal_anim.advance(forgekit_core::FrameTime::ZERO);
        w.reveal_anim
            .advance(forgekit_core::FrameTime::from_nanos(500_000_000));
        let mut rec = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 600.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, with_alpha(scheme.primary_container, 1.0));
        assert_eq!(rec.rrects[1].3, with_alpha(scheme.secondary_container, 1.0));
        assert_eq!(rec.rrects[2].3, with_alpha(scheme.tertiary_container, 1.0));
    }

    #[test]
    fn semantics_reports_closed_trigger_then_open_menu() {
        fn logic_closed(_s: &mut ()) -> FabMenuView<()> {
            fab_menu::<(), _>(leaf_any(24.0, 24.0), false, vec![], |_s: &mut ()| {})
                .label("Compose")
        }
        let mut root: forgekit_core::RenderRoot<(), FabMenuView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic_closed, &mut state);
        let mut tcx = forgekit_text::TextContext::new();
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
        let mut root: forgekit_core::RenderRoot<(), FabMenuView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic_open, &mut state);
        let mut tcx = forgekit_text::TextContext::new();
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
