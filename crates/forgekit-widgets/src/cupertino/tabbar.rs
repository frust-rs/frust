//! `CupertinoTabBar` (Phase 6c, PLAN.md D5, task 13): the iOS bottom tab bar —
//! a 49pt content-height bar of icon+label destinations, systemBlue when
//! selected and secondaryLabel otherwise, with a hairline top separator.
//!
//! `CupertinoTabBarView`/`CupertinoTabBarWidget` are a **controlled** component
//! (see `docs/CODE_STANDARDS.md`'s Interaction Semantics), mirroring
//! [`crate::material::navbar`]: the view reports the *requested* selection
//! through `on_select(index)` and never self-mutates; the app feeds the
//! confirmed `selected` index back in on the next rebuild.
//!
//! Each item is a small retained [`TabItemWidget`] (not `View`/[`AnyView`]-erased
//! — there is only one concrete item type) wrapped in a [`ChildPod`] purely so
//! the bar can route pointer/semantics through the shared
//! [`crate::route_event`]/[`ChildPod::semantics_child`] helpers. Each item lays
//! out an optional caller-supplied `icon` (an opaque [`AnyView`] — tint is the
//! caller's responsibility) above a `label` (a real child [`crate::text`]),
//! and fires `on_select` on release-inside like [`crate::Button`].
//!
//! # Label color
//!
//! An **unselected** label is a child [`crate::text`] themed
//! `on_surface_variant` (iOS secondaryLabel), so it live-swaps with the theme.
//! A **selected** label is painted with an explicit [`SYSTEM_BLUE`] — iOS's
//! systemBlue is *community-measured*, not Apple-published, and has no
//! deferred-color `Text` role to resolve from, so it is baked at build time
//! (a light↔dark systemBlue nuance therefore does not live-swap — an accepted,
//! documented limitation; the widgets' runtime iOS look is a phase-6e item).
//!
//! # Safe area
//!
//! This widget lays out only the 49pt content band. The home-indicator safe-area
//! inset a real iOS tab bar extends into is **shell-future work** (documented,
//! PLAN.md D5) — an app currently supplies its own bottom padding.

use std::rc::Rc;

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, SemanticsCtx, View,
    Widget,
};
use forgekit_text::LineHeight;
use forgekit_theme::Theme;
use kurbo::{Point, Size};
use peniko::Color;

use crate::text;
use crate::text::ThemeTextColor;

/// Content height of the iOS tab bar, in logical px (source: Apple HIG — the
/// standard tab bar content band is 49pt, excluding the home-indicator safe
/// area a shell adds; see the [module docs](self)'s safe-area note).
const HEIGHT: f64 = 49.0;
/// Top inset from the bar's top edge to an item's icon, in logical px.
const PAD_TOP: f64 = 4.0;
/// Gap between an item's icon and its label below it, in logical px.
const LABEL_GAP: f64 = 2.0;

/// The label's iOS *Caption* type-role size, in logical px.
///
/// **Community-approximate**: iOS tab-bar labels render at roughly SF Caption2
/// (~10pt); this is the community-converged size, not an Apple-published tab-bar
/// metric.
const LABEL_SIZE: f32 = 10.0;
const LABEL_LINE_HEIGHT: f32 = 12.0;

/// systemBlue (the selected item tint).
///
/// **Community-measured**: Apple does not publish exact hex for the system
/// accent colors (they vary by trait environment); `#007AFF` is the
/// widely-cited community light value, matching
/// `ColorScheme::cupertino_light().primary`. Baked as an explicit label color
/// (see the [module docs](self)'s Label color note).
const SYSTEM_BLUE: Color = Color::from_rgb8(0x00, 0x7A, 0xFF);

/// Unthemed fallback bar fill (a theme resolves this from `colors.surface`,
/// iOS systemBackground).
const CONTAINER: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`, iOS separator).
const SEPARATOR: Color = Color::from_rgb8(0xC6, 0xC6, 0xC8);
/// The hairline separator's stroke width, in logical px (see
/// [`super::navbar`]'s identical hairline note).
const HAIRLINE_W: f64 = 1.0;

/// The resolved `(container, separator)` colors. Themed: `colors.surface` /
/// `colors.outline_variant`. Unthemed: the [`CONTAINER`]/[`SEPARATOR`] constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => (theme.scheme().surface, theme.scheme().outline_variant),
        None => (CONTAINER, SEPARATOR),
    }
}

/// Build a tab label's type-erased child view. Selected → explicit
/// [`SYSTEM_BLUE`]; unselected → the live `OnSurfaceVariant` (secondaryLabel)
/// role. See the [module docs](self)'s Label color note.
fn label_view<State: 'static>(label: String, selected: bool) -> AnyView<State> {
    let base = text(label)
        .size(LABEL_SIZE)
        .line_height(LineHeight::Absolute(LABEL_LINE_HEIGHT));
    let styled = if selected {
        base.color(SYSTEM_BLUE)
    } else {
        base.themed_role(ThemeTextColor::OnSurfaceVariant)
    };
    forgekit_core::any::<State, _>(styled)
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// One destination's declarative content: an optional leading icon (an opaque
/// [`AnyView`] — tint is the caller's responsibility) and a text label.
pub struct TabItem<State: 'static> {
    icon: Option<AnyView<State>>,
    label: String,
}

/// Create a tab item labelled `label`, with no icon (attach one with
/// [`TabItem::icon`]).
pub fn tab_item<State: 'static>(label: impl Into<String>) -> TabItem<State> {
    TabItem {
        icon: None,
        label: label.into(),
    }
}

impl<State: 'static> TabItem<State> {
    /// Attach a leading icon, erased as an [`AnyView`]. Tint is the supplied
    /// view's own responsibility — see the [module docs](self).
    pub fn icon(mut self, icon: AnyView<State>) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A view-held, typed selection callback (erased per-item on build/rebuild).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative iOS bottom tab bar. See the [module docs](self).
pub struct CupertinoTabBarView<State: 'static> {
    items: Vec<TabItem<State>>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Create a tab bar over `items`, with `selected` the current (app-confirmed)
/// index. Fires `on_select(state, index)` on a release inside an item — a
/// **controlled** component: `selected` is never self-mutated.
pub fn cupertino_tab_bar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<TabItem<State>>,
    selected: usize,
    on_select: F,
) -> CupertinoTabBarView<State> {
    CupertinoTabBarView {
        items,
        selected,
        on_select: Rc::new(on_select),
    }
}

/// PascalCase alias for [`cupertino_tab_bar`].
#[allow(non_snake_case)]
pub fn CupertinoTabBar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<TabItem<State>>,
    selected: usize,
    on_select: F,
) -> CupertinoTabBarView<State> {
    cupertino_tab_bar(items, selected, on_select)
}

/// Erase `on_select` into a per-item callback that always reports `idx`.
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

/// One destination's retained content (see the [module docs](self)).
struct TabItemWidget {
    icon: Option<ChildPod>,
    label: ChildPod,
    label_text: String,
    selected: bool,
    on_select: crate::ErasedCallback,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` — the fire-on-up-inside
    /// contract [`crate::button`] uses.
    captured: bool,
}

/// Build one item's retained [`ChildPod`] (wrapping a fresh [`TabItemWidget`]).
fn build_item<State: 'static>(
    item: &TabItem<State>,
    selected: bool,
    on_select: &OnSelect<State>,
    idx: usize,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let label_view = label_view::<State>(item.label.clone(), selected);
    let icon = item.icon.as_ref().map(|icon| crate::build_child(icon, ctx));
    let label = crate::build_child(&label_view, ctx);
    let widget = TabItemWidget {
        icon,
        label,
        label_text: item.label.clone(),
        selected,
        on_select: item_on_select::<State>(on_select, idx),
        captured: false,
    };
    ChildPod::new(Box::new(widget))
}

impl Widget for TabItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = bc.max().width;
        let mut icon_bottom = PAD_TOP;
        if let Some(icon) = self.icon.as_mut() {
            let icon_size = icon.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, HEIGHT)),
            );
            let icon_x = (width - icon_size.width) / 2.0;
            icon.set_origin(Point::new(icon_x, PAD_TOP));
            icon_bottom = PAD_TOP + icon_size.height;
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(Size::new(width, f64::INFINITY)));
        let label_x = (width - label_size.width) / 2.0;
        self.label
            .set_origin(Point::new(label_x, icon_bottom + LABEL_GAP));
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
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
                ctx.request_redraw();
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
}

/// Synthesize a [`PointerPhase::Cancel`] into a still-armed item pod (mirrors
/// [`crate::material::navbar`]'s `cancel_item`, reimplemented locally since a
/// removed item's `TabItemWidget` isn't `AnyView`-wrapped).
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

/// The retained widget for a [`CupertinoTabBarView`].
pub struct CupertinoTabBarWidget {
    items: Vec<ChildPod>,
}

impl<State: 'static> View<State> for CupertinoTabBarView<State> {
    type Element = CupertinoTabBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoTabBarWidget {
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
        CupertinoTabBarWidget { items }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoTabBarWidget,
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
                .downcast_mut::<TabItemWidget>()
                .expect("tab item pod holds a TabItemWidget");

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

            if prev_item.label != next_item.label || was_selected != now_selected {
                widget.label_text = next_item.label.clone();
                widget.selected = now_selected;
                let prev_view = label_view::<State>(prev_item.label.clone(), was_selected);
                let next_view = label_view::<State>(next_item.label.clone(), now_selected);
                flags |= crate::rebuild_child(&prev_view, &next_view, &mut widget.label, ctx);
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
                    .downcast_mut::<TabItemWidget>()
                    .expect("tab item pod holds a TabItemWidget");
                if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
                    crate::teardown_child(icon_view, icon_pod, ctx);
                }
                let label_view = label_view::<State>(item.label.clone(), idx == prev.selected);
                crate::teardown_child(&label_view, &mut widget.label, ctx);
            }
            element.items.truncate(common);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut CupertinoTabBarWidget, ctx: &mut BuildCtx<'_>) {
        for (i, (item, pod)) in self.items.iter().zip(element.items.iter_mut()).enumerate() {
            let widget = pod
                .widget_mut()
                .downcast_mut::<TabItemWidget>()
                .expect("tab item pod holds a TabItemWidget");
            if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
                crate::teardown_child(icon_view, icon_pod, ctx);
            }
            let label_view = label_view::<State>(item.label.clone(), i == self.selected);
            crate::teardown_child(&label_view, &mut widget.label, ctx);
        }
    }
}

impl Widget for CupertinoTabBarWidget {
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
        let (container, separator) = resolve_colors(Theme::from_paint_ctx(ctx));
        let origin = ctx.origin();
        let size = ctx.size();
        scene.fill_rect(origin, size, container);
        // Hairline along the top edge.
        let y = origin.y + HAIRLINE_W / 2.0;
        scene.stroke_line(
            Point::new(origin.x, y),
            Point::new(origin.x + size.width, y),
            HAIRLINE_W,
            separator,
        );
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
    use crate::test_support::leaf_any;
    use forgekit_core::{BuildCtx, PointerButton, PointerEvent};
    use forgekit_text::TextContext;
    use std::any::Any;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build(view: &CupertinoTabBarView<()>) -> CupertinoTabBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    fn layout(w: &mut CupertinoTabBarWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, bc)
    }

    fn three_items() -> Vec<TabItem<()>> {
        vec![tab_item("Home"), tab_item("Search"), tab_item("Profile")]
    }

    #[test]
    fn layout_divides_width_evenly_and_is_49pt_tall() {
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, HEIGHT));
        assert_eq!(w.items[0].origin().x, 0.0);
        assert_eq!(w.items[1].origin().x, 100.0);
        assert_eq!(w.items[2].origin().x, 200.0);
        assert_eq!(w.items[0].size(), Size::new(100.0, HEIGHT));
    }

    /// A recorder capturing filled rects (with color) and stroked lines.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        lines: Vec<(Point, Point, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, color: Color) {
            self.lines.push((p0, p1, width, color));
        }
    }

    #[test]
    fn unthemed_paint_fills_container_and_draws_top_hairline() {
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].1, Size::new(300.0, HEIGHT));
        assert_eq!(scene.rects[0].2, CONTAINER);
        assert_eq!(scene.lines.len(), 1, "one top hairline");
        assert_eq!(scene.lines[0].3, SEPARATOR);
        // The hairline sits at the top edge.
        assert!(scene.lines[0].0.y < HEIGHT / 2.0);
    }

    #[test]
    fn themed_paint_resolves_surface_and_separator() {
        let theme = Theme::cupertino_baseline();
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, HEIGHT)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0].2, theme.scheme().surface);
        assert_eq!(scene.lines[0].3, theme.scheme().outline_variant);
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut CupertinoTabBarWidget, state: &mut Vec<usize>, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT));
        w.event(&mut ctx, event);
    }

    #[test]
    fn tap_inside_an_item_reports_its_index_without_self_mutating() {
        let view: CupertinoTabBarView<Vec<usize>> = cupertino_tab_bar(
            vec![tab_item("Home"), tab_item("Search"), tab_item("Profile")],
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
    fn up_outside_does_not_fire() {
        let view: CupertinoTabBarView<Vec<usize>> =
            cupertino_tab_bar(three_items_typed(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut log: Vec<usize> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 150.0, 20.0));
        // Release far away (past the item, outside the whole bar height).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 150.0, 500.0));
        assert!(log.is_empty());
    }

    fn three_items_typed() -> Vec<TabItem<Vec<usize>>> {
        vec![tab_item("Home"), tab_item("Search"), tab_item("Profile")]
    }

    #[test]
    fn rebuild_adopts_new_selected_index() {
        let mut counter = 0u64;
        let prev: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        assert!(widget0.selected);

        let next: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 2, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        assert!(!widget0.selected, "selection moved off item 0");
        let widget2 = w.items[2]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        assert!(widget2.selected, "item 2 is now selected");
    }

    #[test]
    fn icon_slot_is_laid_out_above_the_label() {
        let view: CupertinoTabBarView<()> = cupertino_tab_bar(
            vec![tab_item("Home").icon(leaf_any(24.0, 24.0))],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(100.0, 200.0)));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        let icon = widget0.icon.as_ref().expect("icon pod present");
        assert!(icon.origin().y < widget0.label.origin().y);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_state: &mut ()) -> CupertinoTabBarView<()> {
            cupertino_tab_bar(three_items(), 1, |_s: &mut (), _i| {})
        }
        let mut root: forgekit_core::RenderRoot<(), CupertinoTabBarView<()>> =
            forgekit_core::RenderRoot::new();
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
        let selected = tabs
            .iter()
            .find(|(_, n)| n.label() == Some("Search"))
            .expect("the Search tab is present");
        assert_eq!(selected.1.is_selected(), Some(true));
    }
}
