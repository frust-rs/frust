//! The M3 Expressive **floating/docked toolbar** (Phase 6f, PLAN.md Phase B
//! item 5, task 09; RESEARCH.md:84; m3.material.io/components/toolbars/specs;
//! Compose `HorizontalFloatingToolbar`/`DockedToolbar`).
//!
//! [`ToolbarView`]/[`ToolbarWidget`] follow the widget-authoring recipe
//! (`research/RESEARCH.md`'s `widget-authoring-patterns`) and mirror
//! [`super::appbar`]'s slot conventions (read that module first): **leading**,
//! **center**, and **trailing** slots, each an ordered `Vec` of opaque
//! [`AnyView`] children this widget lays out and routes events to but never
//! paints or tints itself — exactly like `AppBarView`'s leading/actions slots,
//! generalized from "one leading + N actions" to "N leading + N center + N
//! trailing" since a toolbar has no mandatory title occupying the center. An
//! optional **fab** slot (`.fab(...)`) always sits at the far trailing edge,
//! after the trailing group — also an opaque, caller-styled [`AnyView`] (a
//! caller typically supplies a real [`super::fab::fab`] here; this widget adds
//! no FAB-specific chrome of its own, the same "tint/style is the caller's
//! job" contract as every other slot).
//!
//! # Variants ([`ToolbarVariant`])
//!
//! * **Floating**: self-sized (hugs its content, like [`super::fab::fab`] —
//!   it does *not* fill the available width), a fully-rounded pill container
//!   (`shape.full`, resolved to a true pill via
//!   [`forgekit_theme::ShapeScale::resolve`] against the bar's own fixed
//!   height) with a `surface_container`-family fill and an M3 elevation
//!   level-2 shadow (`SurfaceContainer` is level 2's own paired
//!   [`forgekit_theme::elevation::SurfaceRole`] in `Elevation::m3()` — the
//!   matching token pair, not an arbitrary pick). "Floating with inset
//!   margins above content" describes the *placement* a caller gives it (a
//!   [`crate::Stack`] with [`crate::Align`]/[`crate::Padding`] around this
//!   self-sized widget, exactly how [`super::fab::fab`] is positioned) — this
//!   widget itself implements no margin/anchoring logic, matching the
//!   established convention that only [`crate::Stack`]/[`crate::Align`]
//!   position a child within extra available space.
//! * **Docked**: fills the available width (`bc.max().width`, like
//!   [`super::appbar`]/[`super::navbar`]), flat geometry (`shape.none`, i.e.
//!   square corners) and no shadow — the "flat" replacement for the static
//!   bottom-app-bar pattern. The leading group hugs the left edge, the
//!   trailing group (+ the fab slot, if any) hugs the right edge in reading
//!   order (mirrors [`super::appbar`]'s reverse-iterate-from-the-edge
//!   layout), and the center group is centered as a whole in the remaining
//!   space between them.
//!
//! All metrics are theme tokens ([`forgekit_theme::ShapeScale`]/
//! [`forgekit_theme::Elevation`]/[`forgekit_theme::ColorScheme`]), each with
//! an unthemed-fallback constant below documenting its themed source. [`GAP`]/
//! [`PAD_X`] have no independently-cited research-ledger figure for this task
//! (marked **community-approximate** per `docs/CODE_STANDARDS.md`).
//!
//! # Semantics
//!
//! The whole bar is one [`Role::Toolbar`] container node (the accesskit role
//! purpose-built for exactly this UI region — chosen over the generic
//! `Role::GenericContainer` [`super::card`] uses, since accesskit publishes a
//! dedicated toolbar role), whose children are every slot pod in visual order
//! (leading, then center, then trailing, then the fab slot if present) via
//! [`forgekit_core::SemanticsCtx::push_container`].
//!
//! # Out of scope (v1)
//!
//! Overflow menus, a vertical floating-toolbar orientation (Compose also
//! ships `VerticalFloatingToolbar` — deferred to a future task), scroll-hide
//! behavior (the Cupertino tab bar's minimize-on-scroll, a separate mechanism
//! task 11 of this phase owns), and catalog exhibits.

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget,
};
use forgekit_theme::{ShapeScale, Theme};
use kurbo::{Point, Size};
use peniko::Color;

/// Fixed container height, in logical px — matches [`super::appbar`]'s
/// `HEIGHT`/[`super::navbar`]'s `HEIGHT` (the same M3 bar-height family both
/// the docked toolbar and the floating toolbar's pill row share).
///
/// Source: m3.material.io/components/toolbars/specs.
const BAR_HEIGHT: f64 = 64.0;

/// Horizontal inset from the bar's leading/trailing edges to the outermost
/// slot content, in logical px (both variants).
///
/// **Community-approximate**: no independently-cited research-ledger figure
/// for this task; mirrors [`super::fab_menu`]'s `EDGE_MARGIN` (itself
/// community-approximate), the closest in-repo precedent for a floating M3X
/// surface's edge inset.
const PAD_X: f64 = 16.0;

/// Gap between adjacent slot elements (within a group, and between groups),
/// in logical px.
///
/// **Community-approximate**: no independently-cited research-ledger figure
/// for this task; scaled up from [`super::appbar`]'s `GAP` (4.0) to suit a
/// toolbar's typically larger action targets.
const GAP: f64 = 8.0;

/// Unthemed-fallback container fill (a theme resolves this from
/// `colors.surface_container`) — matches [`super::navbar`]'s private
/// `CONTAINER` constant exactly (same M3 "surface container" role family).
const CONTAINER: Color = Color::from_rgb8(0xF3, 0xED, 0xF7);

/// Unthemed-fallback floating-variant shadow y-offset, matching
/// `Elevation::m3().level2`'s `y_offset` exactly (`dp / 2.0 + 1.0` at
/// `dp = 3.0`) — matches [`super::fab_menu`]'s shadow constants' derivation,
/// but at level 2 (`SurfaceContainer`, this widget's own container role)
/// rather than level 3.
const FALLBACK_SHADOW_Y_OFFSET: f64 = 2.5;
/// Unthemed-fallback floating-variant shadow blur std-dev, matching
/// `Elevation::m3().level2`.
const FALLBACK_SHADOW_BLUR: f64 = 3.0;
/// Unthemed-fallback floating-variant shadow color (opaque black at
/// `Elevation::m3().level2`'s `0.3` alpha) — matches every other elevated
/// catalog widget's shadow-color derivation (e.g. [`super::card`]'s).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The M3X toolbar container variant. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarVariant {
    /// A self-sized, fully-rounded pill, elevated above content.
    Floating,
    /// A full-width, flat bar docked to an edge (bottom typical).
    Docked,
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved container fill (both variants). Themed:
/// `colors.surface_container`. Unthemed: [`CONTAINER`] exactly.
fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface_container,
        None => CONTAINER,
    }
}

/// The resolved corner radius for `variant`, against the bar's own `width`/
/// `height`. Themed: `shape.full` (floating, resolved to a true pill) /
/// `shape.none` (docked, i.e. square). Unthemed: the equivalent literal
/// tokens ([`f64::INFINITY`]/`0.0`, [`ShapeScale::m3`]'s own values),
/// resolved the same way.
fn resolve_radius(theme: Option<&Theme>, variant: ToolbarVariant, width: f64, height: f64) -> f64 {
    let token = match theme {
        Some(theme) => match variant {
            ToolbarVariant::Floating => theme.shape.full,
            ToolbarVariant::Docked => theme.shape.none,
        },
        None => match variant {
            ToolbarVariant::Floating => f64::INFINITY,
            ToolbarVariant::Docked => 0.0,
        },
    };
    ShapeScale::resolve(token, width, height)
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters at M3
/// elevation level 2 (the floating variant's resting elevation — see the
/// [module docs](self) for why level 2, not another level). Themed:
/// `theme.elevation.level2`'s `ShadowSpec`, colored by `colors.shadow` at the
/// spec's `color_alpha`. Unthemed: the [`FALLBACK_SHADOW_BLUR`]/
/// [`FALLBACK_SHADOW_Y_OFFSET`]/[`FALLBACK_SHADOW_COLOR`] constants exactly.
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = theme.elevation.level2;
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

/// A declarative M3X toolbar. See the [module docs](self).
pub struct ToolbarView<State: 'static> {
    variant: ToolbarVariant,
    leading: Vec<AnyView<State>>,
    center: Vec<AnyView<State>>,
    trailing: Vec<AnyView<State>>,
    fab: Option<AnyView<State>>,
}

/// Create a floating toolbar (self-sized pill, elevated above content) with
/// no slots attached — attach them with [`ToolbarView::leading`]/
/// [`ToolbarView::center`]/[`ToolbarView::trailing`]/[`ToolbarView::fab`].
pub fn floating_toolbar<State: 'static>() -> ToolbarView<State> {
    ToolbarView {
        variant: ToolbarVariant::Floating,
        leading: Vec::new(),
        center: Vec::new(),
        trailing: Vec::new(),
        fab: None,
    }
}

/// PascalCase alias for [`floating_toolbar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn FloatingToolbar<State: 'static>() -> ToolbarView<State> {
    floating_toolbar()
}

/// Create a docked toolbar (full-width, flat bar) with no slots attached —
/// attach them with [`ToolbarView::leading`]/[`ToolbarView::center`]/
/// [`ToolbarView::trailing`]/[`ToolbarView::fab`].
pub fn docked_toolbar<State: 'static>() -> ToolbarView<State> {
    ToolbarView {
        variant: ToolbarVariant::Docked,
        leading: Vec::new(),
        center: Vec::new(),
        trailing: Vec::new(),
        fab: None,
    }
}

/// PascalCase alias for [`docked_toolbar`], matching the container view-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn DockedToolbar<State: 'static>() -> ToolbarView<State> {
    docked_toolbar()
}

impl<State: 'static> ToolbarView<State> {
    /// Attach leading slot children, in reading order (hugs the leading
    /// edge). Tint/styling is each supplied view's own responsibility — see
    /// the [module docs](self).
    pub fn leading(mut self, leading: Vec<AnyView<State>>) -> Self {
        self.leading = leading;
        self
    }

    /// Attach center slot children, in reading order (centered as a group —
    /// docked variant only; the floating variant's row has no extra space to
    /// center within, so the group simply sits between leading and trailing
    /// in visual order). Tint/styling is each supplied view's own
    /// responsibility.
    pub fn center(mut self, center: Vec<AnyView<State>>) -> Self {
        self.center = center;
        self
    }

    /// Attach trailing slot children, in reading order (hugs the trailing
    /// edge, or the fab slot if also attached). Tint/styling is each
    /// supplied view's own responsibility.
    pub fn trailing(mut self, trailing: Vec<AnyView<State>>) -> Self {
        self.trailing = trailing;
        self
    }

    /// Attach the optional prominent/FAB slot, always laid out as the
    /// row's final (rightmost) element — see the [module docs](self) for why
    /// this widget adds no FAB-specific chrome of its own.
    pub fn fab(mut self, fab: AnyView<State>) -> Self {
        self.fab = Some(fab);
        self
    }
}

/// Collect `view`'s leading, center, trailing, and fab (if any) slots into
/// one ordered slice of [`AnyView`] references — the shared shape both
/// `build`/`teardown` and the [`crate::rebuild_children`] reconciliation walk
/// over (mirrors [`super::appbar`]'s `interactive_views`).
fn slot_views<State: 'static>(view: &ToolbarView<State>) -> Vec<&AnyView<State>> {
    let mut views = Vec::with_capacity(
        view.leading.len() + view.center.len() + view.trailing.len() + view.fab.is_some() as usize,
    );
    views.extend(view.leading.iter());
    views.extend(view.center.iter());
    views.extend(view.trailing.iter());
    if let Some(fab) = &view.fab {
        views.push(fab);
    }
    views
}

/// The retained widget for a [`ToolbarView`].
pub struct ToolbarWidget {
    variant: ToolbarVariant,
    /// Every slot pod, in the fixed order [`slot_views`] produces: leading,
    /// then center, then trailing, then the fab slot (if any) — the one list
    /// [`crate::route_event`] hit-tests and [`crate::rebuild_children`]
    /// reconciles.
    slots: Vec<ChildPod>,
    leading_count: usize,
    center_count: usize,
    trailing_count: usize,
    has_fab: bool,
}

impl<State: 'static> View<State> for ToolbarView<State> {
    type Element = ToolbarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToolbarWidget {
        let slots = slot_views(self)
            .into_iter()
            .map(|view| crate::build_child(view, ctx))
            .collect();
        ToolbarWidget {
            variant: self.variant,
            slots,
            leading_count: self.leading.len(),
            center_count: self.center.len(),
            trailing_count: self.trailing.len(),
            has_fab: self.fab.is_some(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToolbarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        let prev_views = slot_views(prev);
        let next_views = slot_views(self);
        flags |= crate::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.slots,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        if element.leading_count != self.leading.len() {
            element.leading_count = self.leading.len();
            flags |= ChangeFlags::LAYOUT;
        }
        if element.center_count != self.center.len() {
            element.center_count = self.center.len();
            flags |= ChangeFlags::LAYOUT;
        }
        if element.trailing_count != self.trailing.len() {
            element.trailing_count = self.trailing.len();
            flags |= ChangeFlags::LAYOUT;
        }
        let now_fab = self.fab.is_some();
        if element.has_fab != now_fab {
            element.has_fab = now_fab;
            flags |= ChangeFlags::LAYOUT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut ToolbarWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in slot_views(self).into_iter().zip(element.slots.iter_mut()) {
            crate::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ToolbarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, BAR_HEIGHT));
        let sizes: Vec<Size> = self
            .slots
            .iter_mut()
            .map(|pod| pod.layout_child(ctx, &slot_bc))
            .collect();

        match self.variant {
            ToolbarVariant::Floating => {
                // Self-sized: one contiguous row (leading, center, trailing,
                // fab), a `GAP` between every adjacent pair of items
                // (naturally collapsing across an empty group's boundary,
                // since the gap is per-item, not per-group) and `PAD_X` at
                // both ends.
                let mut x = PAD_X;
                for (pod, size) in self.slots.iter_mut().zip(sizes.iter()) {
                    pod.set_origin(Point::new(x, (BAR_HEIGHT - size.height) / 2.0));
                    x += size.width + GAP;
                }
                if !self.slots.is_empty() {
                    x -= GAP;
                }
                x += PAD_X;
                bc.constrain(Size::new(x, BAR_HEIGHT))
            }
            ToolbarVariant::Docked => {
                let width = if bc.max().width.is_finite() {
                    bc.max().width
                } else {
                    0.0
                };

                // Leading: left-anchored, sequential (mirrors
                // `appbar.rs`'s leading-slot placement).
                let mut left = PAD_X;
                for (pod, size) in self.slots[..self.leading_count]
                    .iter_mut()
                    .zip(sizes[..self.leading_count].iter())
                {
                    pod.set_origin(Point::new(left, (BAR_HEIGHT - size.height) / 2.0));
                    left += size.width + GAP;
                }
                if self.leading_count > 0 {
                    left -= GAP;
                }

                // Trailing + fab: right-anchored, reverse order — the flat
                // vec's tail (trailing items then the fab slot) lands with
                // the fab flush against the trailing edge, reading order
                // preserved left-to-right (mirrors `appbar.rs`'s
                // reverse-iterate-from-the-edge action layout).
                let tail_start = self.leading_count + self.center_count;
                let mut right = width - PAD_X;
                for (pod, size) in self.slots[tail_start..]
                    .iter_mut()
                    .zip(sizes[tail_start..].iter())
                    .rev()
                {
                    right -= size.width;
                    pod.set_origin(Point::new(right, (BAR_HEIGHT - size.height) / 2.0));
                    right -= GAP;
                }
                if tail_start < self.slots.len() {
                    right += GAP;
                }

                // Center: centered as a group within [left, right].
                let center_range = self.leading_count..tail_start;
                let mut center_total: f64 =
                    sizes[center_range.clone()].iter().map(|s| s.width).sum();
                if center_range.len() > 1 {
                    center_total += GAP * (center_range.len() - 1) as f64;
                }
                let available = (right - left).max(0.0);
                let mut cx = left + ((available - center_total) / 2.0).max(0.0);
                for (pod, size) in self.slots[center_range.clone()]
                    .iter_mut()
                    .zip(sizes[center_range].iter())
                {
                    pod.set_origin(Point::new(cx, (BAR_HEIGHT - size.height) / 2.0));
                    cx += size.width + GAP;
                }

                bc.constrain(Size::new(width, BAR_HEIGHT))
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let o = ctx.origin();
        let size = ctx.size();
        let container = resolve_container(theme);
        let radius = resolve_radius(theme, self.variant, size.width, size.height);

        if self.variant == ToolbarVariant::Floating {
            let (blur, y_offset, shadow_color) = resolve_shadow(theme);
            scene.draw_shadow(
                Point::new(o.x, o.y + y_offset),
                size,
                radius,
                blur,
                shadow_color,
            );
        }

        scene.fill_rounded_rect(o, size, radius, container);

        for pod in &mut self.slots {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::route_event(&mut self.slots, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let slots = &self.slots;
        ctx.push_container(
            Role::Toolbar,
            |_| {},
            |ctx| {
                for pod in slots {
                    pod.semantics_child(ctx);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf_any;
    use forgekit_core::BuildCtx;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    /// Records each `fill_rounded_rect` call's `(origin, size, radius,
    /// color)` — this widget paints its container as a rounded rect, even at
    /// a docked-variant radius of `0.0`, so the shared
    /// [`crate::test_support::RecordingScene`] (which only records
    /// `fill_rect`) can't observe it.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }
    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
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

    #[test]
    fn floating_layout_preserves_slot_order_and_applies_inset_pill_geometry() {
        let view: ToolbarView<()> = floating_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .center(vec![leaf_any(20.0, 20.0)])
            .trailing(vec![leaf_any(24.0, 24.0)])
            .fab(leaf_any(40.0, 40.0));
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(800.0, 200.0)));

        // Self-sized: height fixed, width hugs content (not the loose max).
        assert_eq!(size.height, BAR_HEIGHT);
        assert!(size.width < 800.0);

        // Slot order preserved: leading < center < trailing < fab, left to
        // right.
        assert!(w.slots[0].origin().x < w.slots[1].origin().x);
        assert!(w.slots[1].origin().x < w.slots[2].origin().x);
        assert!(w.slots[2].origin().x < w.slots[3].origin().x);

        // Inset from both edges by PAD_X.
        assert_eq!(w.slots[0].origin().x, PAD_X);
        assert_eq!(
            w.slots[3].origin().x + w.slots[3].size().width,
            size.width - PAD_X
        );

        // A fully-rounded pill: radius resolves to half the fixed bar height
        // (the shorter side), the `ShapeScale::full` clamp.
        let radius = resolve_radius(None, ToolbarVariant::Floating, size.width, size.height);
        assert_eq!(radius, BAR_HEIGHT / 2.0);
    }

    #[test]
    fn docked_layout_spans_the_available_width_with_flat_geometry() {
        let view: ToolbarView<()> = docked_toolbar()
            .leading(vec![leaf_any(24.0, 24.0)])
            .trailing(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        assert_eq!(size, Size::new(400.0, BAR_HEIGHT));

        let radius = resolve_radius(None, ToolbarVariant::Docked, size.width, size.height);
        assert_eq!(radius, 0.0, "docked variant is flat (square corners)");

        // Leading hugs the left edge.
        assert_eq!(w.slots[0].origin().x, PAD_X);
        // Trailing hugs the right edge.
        assert_eq!(
            w.slots[1].origin().x + w.slots[1].size().width,
            400.0 - PAD_X
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

        // Order preserved: leading, then center, then trailing.
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
        layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 200.0)));

        // slots[0] = trailing item, slots[1] = fab.
        assert!(w.slots[0].origin().x < w.slots[1].origin().x);
        assert_eq!(
            w.slots[1].origin().x + w.slots[1].size().width,
            400.0 - PAD_X,
            "the fab slot sits flush against the trailing edge"
        );
    }

    #[test]
    fn unthemed_paint_uses_fallback_container() {
        let view: ToolbarView<()> = docked_toolbar();
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        let mut scene = RRectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, BAR_HEIGHT));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rrects[0].1, Size::new(300.0, BAR_HEIGHT));
        assert_eq!(scene.rrects[0].3, CONTAINER);
    }

    #[test]
    fn themed_paint_resolves_surface_container() {
        let theme = Theme::m3_baseline();
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
    fn semantics_node_is_toolbar_forwarding_every_slot() {
        // A leaf without its own `semantics()` override (e.g. `leaf_any`)
        // contributes no node — use `Text` children instead, so the
        // "forwards every slot" assertion is meaningful (mirrors
        // `card.rs`'s identical semantics-test note).
        fn logic(_state: &mut ()) -> ToolbarView<()> {
            docked_toolbar()
                .leading(vec![forgekit_core::any::<(), _>(crate::text::text(
                    "Leading",
                ))])
                .trailing(vec![forgekit_core::any::<(), _>(crate::text::text(
                    "Trailing",
                ))])
        }
        let mut root: forgekit_core::RenderRoot<(), ToolbarView<()>> =
            forgekit_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = forgekit_text::TextContext::new();
        root.layout_with_text(
            Size::new(300.0, BAR_HEIGHT),
            &mut tcx as &mut dyn std::any::Any,
        );
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Toolbar)
            .expect("a Toolbar node is contributed");
        assert_eq!(node.children().len(), 2, "leading + trailing forwarded");
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
}
