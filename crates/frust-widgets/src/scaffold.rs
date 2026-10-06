//! Scaffold layout container: the four fixed, named chrome slots most screens
//! assemble around — `app_bar`, `body`, `fab`, `bottom_bar` — Flutter's
//! `Scaffold` shape, theme-agnostic (a design system's own bar/nav-bar/FAB
//! widgets plug into the slots from app code; this crate depends on none of
//! them).
//!
//! Only `body` is required ([`scaffold`]); the other three default absent and
//! contribute nothing to layout, paint, or the accessibility tree.
//!
//! # R-B4-inset — a Scaffold consumes no window inset itself
//!
//! The `app_bar` slot is measured at its own natural height *with the
//! status-bar inset already inside it*: a self-sizing bar widget reads
//! `ctx.window_insets().padding().top` in its own `layout` and grows by that
//! amount (`frust_glyph::appbar::AppBarWidget::layout` is the shipped
//! instance). [`ScaffoldWidget`] never pre-insets this slot — doing so would
//! double-consume the same inset. A bar that does **not** self-inset is
//! wrapped by its own author in [`crate::safe_area`], not compensated for
//! here. `body` is likewise never pre-insetted.
//!
//! **`bottom_bar` mirrors the same convention deliberately**, rather than
//! being pre-inset by the bottom window inset the way the v1-design sketch
//! insetted a bare `fab`: it is laid out exactly like `app_bar` (tight width,
//! loose height) and is expected to self-size for `ctx.window_insets()
//! .padding().bottom` the same way a self-insetting `app_bar` does for the
//! top — one inset-consumption convention, two edges. A `bottom_bar` that
//! does not self-inset wraps itself in `safe_area(...).bottom(true)`.
//!
//! `fab` is the one slot the Scaffold *does* inset on the caller's behalf,
//! because nothing else does it and a floating action button has no natural
//! edge to author that logic against: it is aligned within the same box
//! `body` occupies (so `body_behind_app_bar` moves both together), inset by
//! [`ScaffoldView::fab_margin`] on every edge, *plus* the window's bottom
//! inset **only when no `bottom_bar` is present** — a `bottom_bar` has
//! already consumed that inset self-sizing per the paragraph above, so
//! adding it again would double-consume exactly the way a non-self-insetting
//! `app_bar` would. The practical effect matches Flutter: a FAB floats above
//! a bottom bar when both are present, and sits `fab_margin` off the raw
//! window edge (system-inset-aware) when there is no bottom bar to define
//! that edge instead.
//!
//! # Deferred past v1
//!
//! A drawer slot and a snackbar/toast host are **not** part of this widget —
//! an app composes those separately (`overlay_host`, a design system's own
//! toast/sheet widgets) rather than through the Scaffold.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};
use peniko::Color;

use crate::Alignment;

/// Default [`ScaffoldView::fab_margin`] (logical px).
///
/// **Community-approximate**: Material's 16dp FAB screen-edge margin is a
/// common layout convention, not an independently-verified published spec —
/// the same value `plugins/material/src/fab_menu.rs`'s `EDGE_MARGIN` cites
/// for the same reason.
const DEFAULT_FAB_MARGIN: f64 = 16.0;

/// A declarative scaffold. See the [module docs](self).
pub struct ScaffoldView<State: 'static> {
    body: AnyView<State>,
    app_bar: Option<AnyView<State>>,
    bottom_bar: Option<AnyView<State>>,
    fab: Option<AnyView<State>>,
    fab_margin: f64,
    fab_alignment: Alignment,
    body_behind_app_bar: bool,
    background: Option<Color>,
}

/// Scaffold `body` under no chrome — attach `app_bar`/`bottom_bar`/`fab` with
/// the builder methods below.
pub fn scaffold<State: 'static, V: View<State>>(body: V) -> ScaffoldView<State> {
    ScaffoldView {
        body: any(body),
        app_bar: None,
        bottom_bar: None,
        fab: None,
        fab_margin: DEFAULT_FAB_MARGIN,
        // Flutter's `endFloat` default. `Alignment` has no logical start/end
        // concept (the framework carries none anywhere yet), so this is the
        // physical bottom-right corner.
        fab_alignment: Alignment::BOTTOM_RIGHT,
        body_behind_app_bar: false,
        background: None,
    }
}

impl<State: 'static> ScaffoldView<State> {
    /// Attach a top app bar. Self-sizes; see [R-B4-inset](self#r-b4-inset--a-scaffold-consumes-no-window-inset-itself).
    pub fn app_bar<V: View<State>>(mut self, bar: V) -> Self {
        self.app_bar = Some(any(bar));
        self
    }

    /// Conditionally attach a top app bar — `None` clears it. The common
    /// shape for chrome derived from route metadata that isn't always
    /// present. A bare `None` needs the slot's view type spelled out, e.g.
    /// `.app_bar_opt(None::<AnyView<State>>)`.
    ///
    /// ```
    /// use frust_widgets::{ScaffoldView, scaffold, text};
    /// # fn demo(title: Option<&str>) -> ScaffoldView<()> {
    /// scaffold(text("body")).app_bar_opt(title.map(text))
    /// # }
    /// # let _ = demo(Some("Inbox"));
    /// ```
    pub fn app_bar_opt<V: View<State>>(mut self, bar: Option<V>) -> Self {
        self.app_bar = bar.map(any);
        self
    }

    /// Attach a bottom bar. Self-sizes for the bottom window inset the same
    /// way `app_bar` self-sizes for the top one — see the [module docs](self).
    pub fn bottom_bar<V: View<State>>(mut self, bar: V) -> Self {
        self.bottom_bar = Some(any(bar));
        self
    }

    /// Conditionally attach a bottom bar — `None` clears it. Mirrors
    /// [`ScaffoldView::app_bar_opt`].
    pub fn bottom_bar_opt<V: View<State>>(mut self, bar: Option<V>) -> Self {
        self.bottom_bar = bar.map(any);
        self
    }

    /// Attach a floating action button.
    pub fn fab<V: View<State>>(mut self, fab: V) -> Self {
        self.fab = Some(any(fab));
        self
    }

    /// The FAB's inset from every edge of its aligning box (default 16.0,
    /// [`DEFAULT_FAB_MARGIN`]).
    pub fn fab_margin(mut self, margin: f64) -> Self {
        self.fab_margin = margin;
        self
    }

    /// Where the FAB sits within its aligning box (default
    /// [`Alignment::BOTTOM_RIGHT`]).
    pub fn fab_alignment(mut self, alignment: Alignment) -> Self {
        self.fab_alignment = alignment;
        self
    }

    /// Body runs full-height under the bar (the bar floats). Default `false`
    /// (Flutter's `extendBodyBehindAppBar`, inverted default). Does not
    /// affect `bottom_bar` — the body always stops above one when present;
    /// v1 ships no matching `extendBody` toggle.
    pub fn body_behind_app_bar(mut self, enabled: bool) -> Self {
        self.body_behind_app_bar = enabled;
        self
    }

    /// Optional flat fill painted behind every slot. No theme read — the
    /// Scaffold stays theme-agnostic; a themed app supplies its own
    /// `Theme`-resolved color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }
}

/// The retained widget for a [`ScaffoldView`].
pub struct ScaffoldWidget {
    body: ChildPod,
    app_bar: Option<ChildPod>,
    bottom_bar: Option<ChildPod>,
    fab: Option<ChildPod>,
    fab_margin: f64,
    fab_alignment: Alignment,
    body_behind_app_bar: bool,
    background: Option<Color>,
}

/// Reconcile one optional named slot (`app_bar`/`bottom_bar`/`fab`) the same
/// way [`crate::SizedBoxView`]'s single optional child does: build on
/// appearance, tear down on disappearance, reconcile in place otherwise.
/// Shared here because the Scaffold carries three such slots.
fn rebuild_optional_slot<State: 'static>(
    prev: &Option<AnyView<State>>,
    next: &Option<AnyView<State>>,
    slot: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next, &mut *slot) {
        (Some(prev_view), Some(next_view), Some(pod)) => {
            crate::authoring::rebuild_child(prev_view, next_view, pod, ctx)
        }
        (None, Some(next_view), _) => {
            *slot = Some(crate::authoring::build_child(next_view, ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(prev_view), None, Some(pod)) => {
            crate::authoring::teardown_child(prev_view, pod, ctx);
            *slot = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        _ => ChangeFlags::NONE,
    }
}

impl<State: 'static> View<State> for ScaffoldView<State> {
    type Element = ScaffoldWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScaffoldWidget {
        ScaffoldWidget {
            body: crate::authoring::build_child(&self.body, ctx),
            app_bar: self
                .app_bar
                .as_ref()
                .map(|v| crate::authoring::build_child(v, ctx)),
            bottom_bar: self
                .bottom_bar
                .as_ref()
                .map(|v| crate::authoring::build_child(v, ctx)),
            fab: self
                .fab
                .as_ref()
                .map(|v| crate::authoring::build_child(v, ctx)),
            fab_margin: self.fab_margin,
            fab_alignment: self.fab_alignment,
            body_behind_app_bar: self.body_behind_app_bar,
            background: self.background,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScaffoldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        flags |= crate::authoring::rebuild_child(&prev.body, &self.body, &mut element.body, ctx);
        flags |= rebuild_optional_slot(&prev.app_bar, &self.app_bar, &mut element.app_bar, ctx);
        flags |= rebuild_optional_slot(
            &prev.bottom_bar,
            &self.bottom_bar,
            &mut element.bottom_bar,
            ctx,
        );
        flags |= rebuild_optional_slot(&prev.fab, &self.fab, &mut element.fab, ctx);

        if prev.fab_margin != self.fab_margin
            || prev.fab_alignment != self.fab_alignment
            || prev.body_behind_app_bar != self.body_behind_app_bar
        {
            element.fab_margin = self.fab_margin;
            element.fab_alignment = self.fab_alignment;
            element.body_behind_app_bar = self.body_behind_app_bar;
            flags |= ChangeFlags::LAYOUT;
        }
        if prev.background != self.background {
            element.background = self.background;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ScaffoldWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.body, &mut element.body, ctx);
        if let (Some(view), Some(pod)) = (&self.app_bar, &mut element.app_bar) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
        if let (Some(view), Some(pod)) = (&self.bottom_bar, &mut element.bottom_bar) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
        if let (Some(view), Some(pod)) = (&self.fab, &mut element.fab) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
    }
}

/// Map an alignment component in `-1.0..=1.0` to a `0.0..=1.0` fraction of
/// free space — the same mapping [`crate::Align`] uses internally, duplicated
/// here rather than shared (each container keeps its own copy of small
/// positioning predicates, `docs/CODE_STANDARDS.md`'s Interaction Semantics
/// convention for `presses`-shaped helpers).
fn fraction(component: f64) -> f64 {
    (component + 1.0) / 2.0
}

impl Widget for ScaffoldWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        let bounded_height = bc.max().height.is_finite();
        let outer_height = bc.max().height;

        // (1) App bar: tight width, loose (unbounded) height — self-sizes,
        // including any top inset it reads itself (R-B4-inset).
        let bar_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        let bar_height = match &mut self.app_bar {
            Some(pod) => {
                let size = pod.layout_child(ctx, &bar_bc);
                pod.set_origin(Point::ZERO);
                size.height
            }
            None => 0.0,
        };

        // Bottom bar: the same shape, mirrored — self-sizes for the bottom
        // inset (module docs). Origin is set below, once the total height is
        // known.
        let bottom_bar_height = match &mut self.bottom_bar {
            Some(pod) => pod.layout_child(ctx, &bar_bc).height,
            None => 0.0,
        };

        // (2) Body: the remaining space between the bars, full height under
        // `body_behind_app_bar` — never behind `bottom_bar` (v1 has no
        // matching toggle for that edge, module docs).
        let body_top = if self.body_behind_app_bar {
            0.0
        } else {
            bar_height
        };
        let body_bc = if bounded_height {
            let body_height = (outer_height - body_top - bottom_bar_height).max(0.0);
            BoxConstraints::tight(Size::new(width, body_height))
        } else {
            BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY))
        };
        let body_size = self.body.layout_child(ctx, &body_bc);
        self.body.set_origin(Point::new(0.0, body_top));

        let total_height = if bounded_height {
            outer_height
        } else {
            body_top + body_size.height + bottom_bar_height
        };

        if let Some(pod) = &mut self.bottom_bar {
            pod.set_origin(Point::new(0.0, (total_height - bottom_bar_height).max(0.0)));
        }

        // (3) FAB: loose, aligned within the same box the body occupies —
        // see R-B4-inset's fab paragraph for the bottom-inset/bottom_bar
        // interplay this implements.
        let bottom_bar_present = self.bottom_bar.is_some();
        if let Some(pod) = &mut self.fab {
            let fab_size =
                pod.layout_child(ctx, &BoxConstraints::loose(Size::new(width, total_height)));
            let extra_bottom = if bottom_bar_present {
                0.0
            } else {
                ctx.window_insets().padding().bottom
            };
            let box_top = body_top + self.fab_margin;
            let box_left = self.fab_margin;
            let box_right = (width - self.fab_margin).max(box_left);
            let box_bottom =
                (total_height - bottom_bar_height - self.fab_margin - extra_bottom).max(box_top);

            let free_w = (box_right - box_left - fab_size.width).max(0.0);
            let free_h = (box_bottom - box_top - fab_size.height).max(0.0);
            let x = box_left + free_w * fraction(self.fab_alignment.x);
            let y = box_top + free_h * fraction(self.fab_alignment.y);
            pod.set_origin(Point::new(x, y));
        }

        bc.constrain(Size::new(width, total_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(color) = self.background {
            scene.fill_rect(ctx.origin(), ctx.size(), color);
        }
        // Paint order (bottom to top): body, app bar, bottom bar, fab — the
        // fab floats above everything, including a bottom bar.
        self.body.paint_child(ctx, scene);
        if let Some(pod) = &mut self.app_bar {
            pod.paint_child(ctx, scene);
        }
        if let Some(pod) = &mut self.bottom_bar {
            pod.paint_child(ctx, scene);
        }
        if let Some(pod) = &mut self.fab {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Same z-order as `paint`, last = topmost — the four slots aren't a
        // homogeneous `Vec<ChildPod>`, so `authoring::route_event` (which
        // needs a contiguous `&mut [ChildPod]`) doesn't apply; `route_slots`
        // below mirrors its broadcast/focus/capture/hit-test contract over
        // these four discontiguous named fields instead.
        let mut slots: [Option<&mut ChildPod>; 4] = [
            Some(&mut self.body),
            self.app_bar.as_mut(),
            self.bottom_bar.as_mut(),
            self.fab.as_mut(),
        ];
        route_slots(&mut slots, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent structural container: forward every present slot, in
        // the same paint order as above.
        self.body.semantics_child(ctx);
        if let Some(pod) = &self.app_bar {
            pod.semantics_child(ctx);
        }
        if let Some(pod) = &self.bottom_bar {
            pod.semantics_child(ctx);
        }
        if let Some(pod) = &self.fab {
            pod.semantics_child(ctx);
        }
    }

    crate::authoring::visit_children!(body, app_bar, bottom_bar, fab);
}

/// Whether `event` is the phase that auto-releases a recorded capture
/// (`Up`/`Cancel`) — mirrors `authoring`'s private helper of the same shape;
/// duplicated rather than exported, per the crate's per-container-copy
/// convention for small routing predicates (see [`fraction`]'s doc).
fn releases_capture(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
    )
}

/// Whether `event` is a pointer `Down` — the phase that opens a capture and
/// triggers blur-on-outside-tap below.
fn is_pointer_down(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Down)
    )
}

/// Route an event across the scaffold's four fixed named slots, in `slots`'
/// z-order (last = topmost) — the same broadcast/focus-routed/capture/
/// hit-test contract as [`crate::authoring::route_event`], adapted to a
/// discontiguous set of named fields (that helper needs one contiguous
/// `&mut [ChildPod]`, which four separately-typed struct fields cannot
/// supply).
fn route_slots(
    slots: &mut [Option<&mut ChildPod>; 4],
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if event.is_broadcast() {
        for slot in slots.iter_mut() {
            if let Some(pod) = slot.as_deref_mut() {
                pod.event_child(ctx, event);
            }
        }
        return EventResult::Ignored;
    }
    if event.is_focus_routed() {
        for slot in slots.iter_mut() {
            if let Some(pod) = slot.as_deref_mut()
                && pod.is_focused()
            {
                return pod.event_child(ctx, event);
            }
        }
        return EventResult::Ignored;
    }
    for slot in slots.iter_mut() {
        if let Some(pod) = slot.as_deref_mut()
            && pod.is_active()
        {
            let result = pod.event_child(ctx, event);
            if releases_capture(event) {
                pod.set_active(false);
            }
            return result;
        }
    }

    let position = event.position();
    let mut handled = EventResult::Ignored;
    let mut kept_focus: Option<usize> = None;
    for (i, slot) in slots.iter_mut().enumerate().rev() {
        if let Some(pod) = slot.as_deref_mut()
            && pod.contains(position)
            && pod.event_child(ctx, event) == EventResult::Handled
        {
            if pod.is_focused() {
                kept_focus = Some(i);
            }
            handled = EventResult::Handled;
            break;
        }
    }
    if is_pointer_down(event) {
        for (i, slot) in slots.iter_mut().enumerate() {
            if Some(i) != kept_focus
                && let Some(pod) = slot.as_deref_mut()
                && pod.is_focused()
            {
                pod.set_focused(false);
            }
        }
    }
    handled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use frust_core::BuildCtx;

    fn build<S: 'static>(view: &ScaffoldView<S>) -> ScaffoldWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn body_only_fills_the_whole_box() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        assert_eq!(size, Size::new(300.0, 400.0));
        assert_eq!(w.body.origin(), Point::ZERO);
        assert_eq!(w.body.size(), Size::new(300.0, 400.0));
        assert!(w.app_bar.is_none());
        assert!(w.bottom_bar.is_none());
        assert!(w.fab.is_none());
    }

    #[test]
    fn app_bar_offsets_the_body_by_its_own_height() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0)).app_bar(leaf(0.0, 56.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        assert_eq!(w.app_bar.as_ref().unwrap().origin(), Point::ZERO);
        assert_eq!(w.app_bar.as_ref().unwrap().size().height, 56.0);
        assert_eq!(w.body.origin(), Point::new(0.0, 56.0));
        assert_eq!(w.body.size(), Size::new(300.0, 344.0));
    }

    #[test]
    fn body_behind_app_bar_gives_the_body_full_height() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0))
            .app_bar(leaf(0.0, 56.0))
            .body_behind_app_bar(true);
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        assert_eq!(w.body.origin(), Point::ZERO);
        assert_eq!(w.body.size(), Size::new(300.0, 400.0));
    }

    #[test]
    fn bottom_bar_sits_at_the_bottom_edge_and_body_stops_above_it() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0)).bottom_bar(leaf(0.0, 48.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        let bar = w.bottom_bar.as_ref().unwrap();
        assert_eq!(bar.size().height, 48.0);
        assert_eq!(bar.origin(), Point::new(0.0, 400.0 - 48.0));
        assert_eq!(w.body.origin(), Point::ZERO);
        assert_eq!(w.body.size(), Size::new(300.0, 400.0 - 48.0));
    }

    #[test]
    fn fab_defaults_to_bottom_right_inset_by_margin() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0)).fab(leaf(56.0, 56.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        let fab = w.fab.as_ref().unwrap();
        // No window inset in `LayoutCtx::new()`'s default; no bottom bar —
        // just `fab_margin` off each edge.
        assert_eq!(
            fab.origin(),
            Point::new(300.0 - 16.0 - 56.0, 400.0 - 16.0 - 56.0)
        );
    }

    #[test]
    fn fab_floats_above_a_present_bottom_bar_with_no_double_inset() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0))
            .bottom_bar(leaf(0.0, 48.0))
            .fab(leaf(56.0, 56.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        let fab = w.fab.as_ref().unwrap();
        // The fab's box stops above the 48px bottom bar; margin only (no
        // window inset added on top of the bar's own self-sizing).
        assert_eq!(
            fab.origin(),
            Point::new(300.0 - 16.0 - 56.0, 400.0 - 48.0 - 16.0 - 56.0)
        );
    }

    #[test]
    fn all_four_slots_together() {
        let view: ScaffoldView<()> = scaffold(leaf(50.0, 50.0))
            .app_bar(leaf(0.0, 56.0))
            .bottom_bar(leaf(0.0, 48.0))
            .fab(leaf(56.0, 56.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(300.0, 400.0)));
        assert_eq!(w.body.origin(), Point::new(0.0, 56.0));
        assert_eq!(w.body.size(), Size::new(300.0, 400.0 - 56.0 - 48.0));
        assert_eq!(
            w.bottom_bar.as_ref().unwrap().origin(),
            Point::new(0.0, 400.0 - 48.0)
        );
        assert_eq!(
            w.fab.as_ref().unwrap().origin(),
            Point::new(300.0 - 16.0 - 56.0, 400.0 - 48.0 - 16.0 - 56.0)
        );
    }
}
