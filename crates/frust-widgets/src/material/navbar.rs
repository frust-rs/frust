//! The M3 bottom `NavigationBar`: 64dp current-baseline height (androidx
//! `NavigationBarTokens`; the pre-Expressive 80dp value is noted as a
//! constant), using the shared [`super::state_layer`] interaction overlay on
//! each destination.
//!
//! `NavigationBarView`/`NavigationBarWidget` follow the widget-authoring
//! recipe: a **controlled** component — [`NavigationBarView`] reports the
//! *requested* selection through `on_select(index)` and never self-mutates; the app
//! feeds the confirmed `selected` index back in on the next rebuild, exactly
//! like [`crate::Checkbox`]/[`crate::Slider`].
//!
//! Each item is a small, self-contained retained widget
//! ([`NavItemWidget`], **not** built through `View`/[`frust_core::AnyView`]
//! — there is only ever one concrete item type, so no type erasure is
//! needed) wrapped in its own [`frust_core::ChildPod`] purely so the bar
//! can route pointer/semantics through the same
//! [`crate::route_event`]/[`frust_core::ChildPod::semantics_child`]
//! helpers every multi-child container uses. Each item:
//!
//! * lays out an optional caller-supplied `icon` (an opaque `AnyView` — tint
//!   is the caller's own responsibility, mirroring [`super::appbar`]'s
//!   leading/actions slots) above a `label` (a real child [`crate::text`],
//!   themed `onSurface` when selected / `onSurfaceVariant` otherwise);
//! * paints its own selection indicator (a `secondaryContainer`-filled pill
//!   behind the icon), whose opacity is driven by a per-item
//!   [`frust_core::AnimationController`] sprung via the M3 Expressive
//!   `default_effects` token (critically damped — see [`DEFAULT_EFFECTS_SPRING`])
//!   on every selection change, advanced during its own `paint` (see
//!   `docs/ARCHITECTURE.md`'s Frame pipeline);
//! * fires `on_select` on release-inside, fire-on-up-inside like
//!   [`crate::Button`].
//!
//! # Semantics
//!
//! The bar is one [`Role::TabList`] container node; each item contributes a
//! [`Role::Tab`] node labelled with its text and carrying
//! [`accesskit::Node::set_selected`] — the closest accesskit vocabulary to
//! "one of a set of mutually-exclusive destinations", chosen over
//! `Role::RadioGroup`/`Role::Toggled` since a bottom nav bar reads to a
//! screen reader exactly like a tab strip.

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnimationController, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx,
    EventResult, FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, SemanticsCtx, SpringDesc, View, Widget,
};
use frust_text::{FontWeight, LineHeight};
use frust_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

use crate::text;
use crate::text::ThemeTextColor;

/// Container height of the current Expressive baseline, in logical px.
///
/// Source: androidx `NavigationBarTokens` (material-components-android
/// `docs/components/BottomNavigation.md`). The pre-Expressive M3 "standard"
/// height was **80dp** — superseded (this widget ships only the current
/// 64dp baseline; the legacy value is noted here for the historical record,
/// not a second constant this module reads).
const HEIGHT: f64 = 64.0;
/// Selection indicator pill width, logical px (androidx `NavigationBarTokens`
/// "active indicator" width).
const INDICATOR_W: f64 = 56.0;
/// Selection indicator pill height, logical px.
const INDICATOR_H: f64 = 32.0;
/// Top inset from the bar's top edge to the indicator pill's top edge.
const PAD_TOP: f64 = 8.0;
/// Gap between the indicator/icon area and the label below it.
const LABEL_GAP: f64 = 4.0;

/// The label's M3 `labelMediumEmphasized` type-scale token (see
/// [`super::appbar`]'s `TITLE_SIZE` doc comment for why this is a hardcoded
/// constant rather than a live `Theme::type_scale` read — `Text` only defers
/// *color* resolution past `View::build`, never size/weight). Matches
/// `frust-theme::typography`'s `LABEL_MEDIUM_EMPHASIZED` token: same
/// size/line-height/letter-spacing as the baseline `LABEL_MEDIUM`, weight
/// stepped up from Medium to Bold (the emphasized-type consumption — was
/// `FontWeight::MEDIUM` previously).
const LABEL_SIZE: f32 = 12.0;
const LABEL_LINE_HEIGHT: f32 = 16.0;
const LABEL_LETTER_SPACING: f32 = 0.5;
const LABEL_WEIGHT: FontWeight = FontWeight::BOLD;

/// M3 Expressive `default_effects` spring token (mass 1.0, stiffness 1600.0,
/// critically damped — opacity/color motion never overshoots). Hardcoded for
/// the same reason as the label's type-scale tokens above: an
/// `AnimationController`'s spring parameters have no deferred, post-`build`
/// resolution seam the way a themed *color* does. Matches
/// `frust-theme::motion::MotionScheme::m3_expressive().default_effects`.
const DEFAULT_EFFECTS_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 1600.0,
    damping_ratio: 1.0,
};

/// Unthemed fallback container fill (a theme resolves this from
/// `colors.surface_container`).
const CONTAINER: Color = Color::from_rgb8(0xF3, 0xED, 0xF7);
/// Unthemed fallback indicator pill fill (a theme resolves this from
/// `colors.secondary_container`).
const INDICATOR: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);

/// The resolved `(container, indicator)` fills. Themed:
/// `surface_container`/`secondary_container`. Unthemed: the
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

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Build a label's type-erased child view at `labelMediumEmphasized`, tagged
/// with the themed color `role` selection determines (`OnSurface` selected /
/// `OnSurfaceVariant` unselected).
fn label_view<State: 'static>(label: String, role: ThemeTextColor) -> AnyView<State> {
    frust_core::any::<State, _>(
        text(label)
            .size(LABEL_SIZE)
            .weight(LABEL_WEIGHT)
            .letter_spacing(LABEL_LETTER_SPACING)
            .line_height(LineHeight::Absolute(LABEL_LINE_HEIGHT))
            .themed_role(role),
    )
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (mirrors [`crate::button`]'s helper of the same shape).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// One destination's declarative content: an optional leading icon (an
/// opaque [`AnyView`] — tint is the caller's own responsibility) and a text
/// label.
pub struct NavItem<State: 'static> {
    icon: Option<AnyView<State>>,
    label: String,
}

/// Create a nav item labelled `label`, with no icon (attach one with
/// [`NavItem::icon`]).
pub fn nav_item<State: 'static>(label: impl Into<String>) -> NavItem<State> {
    NavItem {
        icon: None,
        label: label.into(),
    }
}

impl<State: 'static> NavItem<State> {
    /// Attach a leading icon, erased as an [`AnyView`]. Tint is the supplied
    /// view's own responsibility — see the [module docs](self).
    pub fn icon(mut self, icon: AnyView<State>) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A view-held, typed selection callback (erased per-item on build/rebuild).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative M3 bottom NavigationBar. See the [module docs](self).
pub struct NavigationBarView<State: 'static> {
    items: Vec<NavItem<State>>,
    selected: usize,
    on_select: OnSelect<State>,
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
        items,
        selected,
        on_select: Rc::new(on_select),
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

/// Erase `on_select` into a per-item callback that always reports `idx`
/// (mirrors [`crate::erase_callback_arg`], but with the index closed over
/// rather than passed at call time — every item needs its *own* fixed index).
fn item_on_select<State: 'static>(
    on_select: &OnSelect<State>,
    idx: usize,
) -> crate::ErasedCallback {
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
    label: ChildPod,
    /// The label text, retained for the semantics node's label.
    label_text: String,
    selected: bool,
    /// Drives the selection-indicator pill's fade in/out; sprung via
    /// [`DEFAULT_EFFECTS_SPRING`] on every selection change (see
    /// [`NavigationBarWidget`]'s `rebuild`).
    indicator: AnimationController,
    on_select: crate::ErasedCallback,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` — see [`crate::button`]'s
    /// identical fire-on-up-inside contract.
    captured: bool,
}

/// Build one item's retained [`ChildPod`] (wrapping a fresh [`NavItemWidget`]).
///
/// `selected` seeds the indicator pill's *settled* opacity with no fade-in on
/// first mount (a zero-duration [`AnimationController::animate_to`] resolved
/// via one throwaway [`AnimationController::advance`] — selection changes
/// after this first build animate via [`DEFAULT_EFFECTS_SPRING`] instead, see
/// `rebuild`).
fn build_item<State: 'static>(
    item: &NavItem<State>,
    selected: bool,
    on_select: &OnSelect<State>,
    idx: usize,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let role = if selected {
        ThemeTextColor::OnSurface
    } else {
        ThemeTextColor::OnSurfaceVariant
    };
    let label_view = label_view::<State>(item.label.clone(), role);
    let icon = item.icon.as_ref().map(|icon| crate::build_child(icon, ctx));
    let label = crate::build_child(&label_view, ctx);

    let mut indicator = AnimationController::new(Duration::ZERO);
    if selected {
        indicator.animate_to(1.0);
        indicator.advance(FrameTime::ZERO); // zero-duration: settles instantly.
    }

    let widget = NavItemWidget {
        icon,
        label,
        label_text: item.label.clone(),
        selected,
        indicator,
        on_select: item_on_select::<State>(on_select, idx),
        pressed: false,
        captured: false,
    };
    ChildPod::new(Box::new(widget))
}

impl Widget for NavItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = bc.max().width;
        if let Some(icon) = self.icon.as_mut() {
            let icon_size = icon.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, INDICATOR_H)),
            );
            let icon_x = (width - icon_size.width) / 2.0;
            let icon_y = PAD_TOP + (INDICATOR_H - icon_size.height) / 2.0;
            icon.set_origin(Point::new(icon_x, icon_y));
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(Size::new(width, f64::INFINITY)));
        let label_x = (width - label_size.width) / 2.0;
        let label_y = PAD_TOP + INDICATOR_H + LABEL_GAP;
        self.label.set_origin(Point::new(label_x, label_y));
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.indicator.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let opacity = self.indicator.value_clamped() as f32;
        if opacity > 0.0 {
            let (_, indicator_fill) = resolve_colors(Theme::from_paint_ctx(ctx));
            let width = ctx.size().width;
            let pill_w = INDICATOR_W.min(width);
            let pill_x = ctx.origin().x + (width - pill_w) / 2.0;
            let pill_y = ctx.origin().y + PAD_TOP;
            scene.fill_rounded_rect(
                Point::new(pill_x, pill_y),
                Size::new(pill_w, INDICATOR_H),
                INDICATOR_H / 2.0,
                with_alpha(indicator_fill, opacity),
            );
        }
        if let Some(icon) = self.icon.as_mut() {
            icon.paint_child(ctx, scene);
        }
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = inside(p.position, ctx.size());
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (self.on_select)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
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
}

/// Synthesize a [`PointerPhase::Cancel`] into a still-armed item pod (mirrors
/// [`crate`]'s crate-private `cancel_pod`, reimplemented locally since a
/// removed item's `NavItemWidget` isn't `AnyView`-wrapped and so can't go
/// through [`crate::teardown_child`]'s `Box<dyn Widget>` downcast).
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

/// The retained widget for a [`NavigationBarView`].
pub struct NavigationBarWidget {
    /// Each destination, in order — every pod wraps a [`NavItemWidget`].
    items: Vec<ChildPod>,
}

impl<State: 'static> View<State> for NavigationBarView<State> {
    type Element = NavigationBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> NavigationBarWidget {
        let mut items = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.iter().enumerate() {
            items.push(build_item(
                item,
                i == self.selected,
                &self.on_select,
                i,
                ctx,
            ));
        }
        NavigationBarWidget { items }
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
                    flags |= crate::rebuild_child(
                        p,
                        n,
                        widget.icon.as_mut().expect("icon pod present"),
                        ctx,
                    );
                }
                (None, Some(n)) => {
                    widget.icon = Some(crate::build_child(n, ctx));
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (Some(p), None) => {
                    if let Some(mut old) = widget.icon.take() {
                        crate::teardown_child(p, &mut old, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (None, None) => {}
            }

            let prev_role = if was_selected {
                ThemeTextColor::OnSurface
            } else {
                ThemeTextColor::OnSurfaceVariant
            };
            let next_role = if now_selected {
                ThemeTextColor::OnSurface
            } else {
                ThemeTextColor::OnSurfaceVariant
            };
            if prev_item.label != next_item.label || prev_role != next_role {
                widget.label_text = next_item.label.clone();
                let prev_view = label_view::<State>(prev_item.label.clone(), prev_role);
                let next_view = label_view::<State>(next_item.label.clone(), next_role);
                flags |= crate::rebuild_child(&prev_view, &next_view, &mut widget.label, ctx);
            }

            if was_selected != now_selected {
                widget.selected = now_selected;
                if now_selected {
                    widget.indicator.fling(0.0, DEFAULT_EFFECTS_SPRING);
                } else {
                    // A negative velocity targets 0.0 (`fling` only checks the
                    // sign) — `-0.0001` rather than `-0.0`, since IEEE `-0.0 >=
                    // 0.0` is `true` and would target `1.0` instead.
                    widget.indicator.fling(-0.0001, DEFAULT_EFFECTS_SPRING);
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
                if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
                    crate::teardown_child(icon_view, icon_pod, ctx);
                }
                let role = if idx == prev.selected {
                    ThemeTextColor::OnSurface
                } else {
                    ThemeTextColor::OnSurfaceVariant
                };
                let label_view = label_view::<State>(item.label.clone(), role);
                crate::teardown_child(&label_view, &mut widget.label, ctx);
            }
            element.items.truncate(common);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut NavigationBarWidget, ctx: &mut BuildCtx<'_>) {
        for (i, (item, pod)) in self.items.iter().zip(element.items.iter_mut()).enumerate() {
            let widget = pod
                .widget_mut()
                .downcast_mut::<NavItemWidget>()
                .expect("nav item pod holds a NavItemWidget");
            if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
                crate::teardown_child(icon_view, icon_pod, ctx);
            }
            let role = if i == self.selected {
                ThemeTextColor::OnSurface
            } else {
                ThemeTextColor::OnSurfaceVariant
            };
            let label_view = label_view::<State>(item.label.clone(), role);
            crate::teardown_child(&label_view, &mut widget.label, ctx);
        }
    }
}

impl Widget for NavigationBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let n = self.items.len();
        if n == 0 {
            return bc.constrain(Size::new(width, HEIGHT));
        }
        let slot_w = width / n as f64;
        for (i, pod) in self.items.iter_mut().enumerate() {
            pod.layout_child(ctx, &BoxConstraints::tight(Size::new(slot_w, HEIGHT)));
            pod.set_origin(Point::new(i as f64 * slot_w, 0.0));
        }
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (container_fill, _) = resolve_colors(Theme::from_paint_ctx(ctx));
        scene.fill_rect(ctx.origin(), ctx.size(), container_fill);
        for pod in &mut self.items {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::route_event(&mut self.items, ctx, event)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf_any};
    use frust_core::{BuildCtx, PointerButton, PointerEvent};
    use frust_text::TextContext;
    use std::any::Any;

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

    fn three_items() -> Vec<NavItem<()>> {
        vec![nav_item("Home"), nav_item("Search"), nav_item("Profile")]
    }

    #[test]
    fn layout_divides_width_evenly_and_fills_height() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, HEIGHT));
        assert_eq!(w.items[0].origin().x, 0.0);
        assert_eq!(w.items[1].origin().x, 100.0);
        assert_eq!(w.items[2].origin().x, 200.0);
        assert_eq!(w.items[0].size(), Size::new(100.0, HEIGHT));
    }

    #[test]
    fn unthemed_paint_uses_fallback_container() {
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, HEIGHT));
    }

    #[test]
    fn themed_paint_resolves_surface_container() {
        let theme = frust_theme::Theme::m3_baseline();
        let view: NavigationBarView<()> = navigation_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));

        struct ColorRecorder {
            colors: Vec<Color>,
        }
        impl PaintScene for ColorRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, c: Color) {
                self.colors.push(c);
            }
            fn draw_text(&mut self, _o: Point, _t: &str) {}
        }
        let mut scene = ColorRecorder { colors: Vec::new() };
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.colors[0], theme.scheme().surface_container);
    }

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
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT));
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
        // The newly-selected item's indicator started animating (a spring
        // fling toward 1.0, not an instant snap).
        assert!(widget2.indicator.is_animating());
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
        assert!(icon.origin().y < widget0.label.origin().y);
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
        root.layout_with_text(Size::new(300.0, HEIGHT), &mut tcx as &mut dyn Any);
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
}
