//! `context_menu`: the dropdown menu's panel, opened at the pointer by a
//! secondary click on a trigger region.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/context-menu.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `ContextMenu` whose content, items, labels, separators, shortcuts and
//! sub-content carry the *same* class lists as `dropdown-menu.tsx`'s, so this
//! module ports only what differs: the trigger, and where the panel is anchored.
//!
//! # The trigger anchors a point, not a box
//!
//! Radix anchors a context menu to the **pointer position** (its virtual
//! reference element), not to the trigger's box. [`context_menu_trigger`]
//! reproduces that: a secondary-button press latches the press point, and the
//! trigger writes a zero-size anchor rect at that point (in window space) on its
//! next paint — the one pass that knows its own absolute origin. The host then
//! places the panel below-trailing of that point with no side offset, exactly as
//! Radix does. Before any secondary press the trigger publishes its own box, so
//! a menu opened some other way still anchors somewhere sensible.
//!
//! The anchor cell is not reactive (see [`OverlayAnchor`]), so the frame that
//! writes the press point is the frame after the press, and the host re-places
//! itself once it sees the anchor move — the same one-frame self-correction any
//! moved trigger takes.
//!
//! # Secondary-button reach
//!
//! `PointerEvent::button` carries [`PointerButton::Secondary`], which is what
//! this trigger matches on: on desktop, `frust-shell-desktop` forwards the
//! right mouse button as a secondary press, so a right-click over the trigger
//! area opens the menu at the pointer.
//!
//! Touch has no secondary button at all, and long-press-opens-a-context-menu is
//! not modelled: a touch device sees no context menu from this component.

use std::cell::RefCell;
use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerButton, PointerPhase,
    Rect, SemanticsCtx, Size, View, Widget, any, build_child, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child, visit_children,
};

use crate::components::dropdown_menu::{DropdownMenuItem, MenuListStyle, menu_panel};
use crate::components::popover::{OnOpenChange, PanelHandle, PanelStyle};
use crate::hit::inside;
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};

/// A declarative shadcn context menu. See [`context_menu`].
pub struct ContextMenuView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    style: PanelHandle,
    placement: OverlayPlacement,
}

/// Build a context-menu panel over `items`, to be mounted while the app's own
/// open flag is set — or kept mounted with the flag handed to
/// [`ContextMenuView::open`] for an exit ramp (see [`crate::popover`] for both
/// mount contracts).
///
/// `on_select(state, index)` reports an activation with the same depth-first
/// index [`crate::dropdown_menu`] documents — the two share their whole list.
pub fn context_menu<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<DropdownMenuItem>,
    on_select: F,
) -> ContextMenuView<State> {
    let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::menu()));
    let content = menu_panel(
        items,
        MenuListStyle::menu(),
        style.clone(),
        Rc::new(on_select),
    );
    // Anchored to a *point*: below and trailing of it, flush against it.
    let placement = OverlayPlacement::on(OverlaySide::Bottom)
        .align(OverlayAlign::Start)
        .offset(0.0);
    ContextMenuView {
        inner: anchored(content).placement(placement),
        style,
        placement,
    }
}

impl<State: 'static> ContextMenuView<State> {
    /// Anchor the menu to the point (or box) `anchor` carries — the cell the
    /// trigger writes.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the menu opens on (default `bottom`, from the press point).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Hand a **kept-mounted** menu the app's open flag, so closing it plays the
    /// panel's exit ramp instead of vanishing (see [`crate::popover`]).
    ///
    /// The default is `true`: a mounted menu is an open one.
    pub fn open(mut self, open: bool) -> Self {
        self.style.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: a press outside the menu or a focus-routed
    /// Escape reports `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }
}

impl<State: 'static> View<State> for ContextMenuView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

/// Wrap `child` as a context-menu trigger region: a secondary press inside it
/// reports `on_open_change(state, true)` and publishes the press point into
/// `anchor`.
///
/// Transparent in every other respect — it lays out, paints, routes and publishes
/// the semantics of `child` unchanged, and a primary press passes straight
/// through to whatever is inside it.
pub fn context_menu_trigger<State: 'static, V: View<State>>(
    anchor: &OverlayAnchor,
    child: V,
) -> ContextMenuTriggerView<State> {
    ContextMenuTriggerView {
        child: any(child),
        anchor: anchor.clone(),
        on_open_change: Rc::new(|_, _| {}),
    }
}

/// A declarative context-menu trigger region. See [`context_menu_trigger`].
pub struct ContextMenuTriggerView<State: 'static> {
    child: AnyView<State>,
    anchor: OverlayAnchor,
    on_open_change: OnOpenChange<State>,
}

impl<State: 'static> ContextMenuTriggerView<State> {
    /// Set the open-change callback: `true` on a secondary press inside.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// The retained widget for a [`ContextMenuTriggerView`].
pub struct ContextMenuTriggerWidget {
    child: ChildPod,
    anchor: OverlayAnchor,
    /// The latched press point, in this widget's own space, published to the
    /// anchor on the next paint (see the [module docs](self)).
    point: Option<Point>,
    on_open_change: ErasedArgCallback<bool>,
}

impl ContextMenuTriggerWidget {
    /// The press point this trigger will publish, in its own space.
    pub fn press_point(&self) -> Option<Point> {
        self.point
    }
}

impl<State: 'static> View<State> for ContextMenuTriggerView<State> {
    type Element = ContextMenuTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ContextMenuTriggerWidget {
        ContextMenuTriggerWidget {
            child: build_child(&self.child, ctx),
            anchor: self.anchor.clone(),
            point: None,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ContextMenuTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        element.anchor = self.anchor.clone();
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut ContextMenuTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ContextMenuTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::origin` is absolute window space — the one read that turns a
        // widget-local press point into the window-space anchor the host needs.
        let origin = ctx.origin();
        match self.point {
            Some(p) => self
                .anchor
                .set(Rect::from_origin_size(origin + p.to_vec2(), Size::ZERO)),
            None => self.anchor.set(Rect::from_origin_size(origin, ctx.size())),
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A secondary press is claimed before the child sees it — the region owns
        // that button, and no baseline control does anything with it — while
        // every other pass, including the whole primary gesture, is the child's.
        if let InputEvent::Pointer(p) = event
            && p.button == PointerButton::Secondary
            && p.phase == PointerPhase::Down
            && inside(p.position, ctx.size())
        {
            self.point = Some(p.position);
            // Focus is what routes Escape to the menu's host afterwards.
            ctx.request_focus();
            (self.on_open_change)(ctx, true);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::dropdown_menu::{dropdown_menu_item, dropdown_menu_separator};
    use crate::components::popover::tests::{Recorder, WINDOW, escape, ft_ms, light, pointer};
    use frust::authoring::PointerEvent;
    use frust::authoring::text::TextContext;
    use frust::{Padding, SizedBox};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        open: bool,
        opens: Vec<bool>,
        selected: Vec<usize>,
    }

    fn items() -> Vec<DropdownMenuItem> {
        vec![
            dropdown_menu_item("Back"),
            dropdown_menu_item("Forward").disabled(true),
            dropdown_menu_separator(),
            dropdown_menu_item("Reload").shortcut("⌘R"),
        ]
    }

    /// A secondary-button pointer event at `(x, y)`.
    fn secondary(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    /// The trigger region inset inside the window, so the published anchor point
    /// proves it is absolute rather than region-local.
    const INSET: f64 = 24.0;

    /// `duration-200`, the shared ramp the panel exits over.
    const RAMP_MS: f64 = 200.0;

    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
        /// Whether the menu is kept mounted and handed the flag (the mount an
        /// exit ramp needs) rather than mounted only while open.
        kept: bool,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_mount(false)
        }

        fn kept_mounted() -> Self {
            Self::with_mount(true)
        }

        fn with_mount(kept: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor: OverlayAnchor::new(),
                kept,
                clock: 0.0,
            };
            h.root.set_theme(Box::new(light()));
            h.pass();
            h
        }

        fn pass(&mut self) {
            let anchor = self.anchor.clone();
            let kept = self.kept;
            let mut logic = move |state: &mut AppState| {
                let region = Padding(
                    frust::EdgeInsets::all(INSET),
                    context_menu_trigger(&anchor, SizedBox(Some(200.0), Some(200.0)))
                        .on_open_change(|s: &mut AppState, open| {
                            s.opens.push(open);
                            s.open = open;
                        }),
                );
                let mut children = vec![any(region)];
                if kept || state.open {
                    children.push(any(context_menu(items(), |s: &mut AppState, i| {
                        s.selected.push(i)
                    })
                    .anchor(&anchor)
                    .open(state.open)
                    .on_open_change(|s: &mut AppState, open| {
                        s.opens.push(open);
                        s.open = open;
                    })));
                }
                frust::Stack(children)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        /// Paint at `ms` without rebuilding — the ramp's own frames.
        fn paint_at(&mut self, ms: f64) -> Recorder {
            self.clock = ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        /// Whether the menu's `bg-popover` panel was drawn.
        fn panel_painted(rec: &Recorder) -> bool {
            let theme = light();
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().surface_container_high)
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
            self.pass();
        }
    }

    #[test]
    fn a_secondary_press_opens_the_menu_and_anchors_it_at_the_pointer() {
        let mut h = Harness::new();
        assert_eq!(
            h.anchor.rect(),
            Rect::from_origin_size(Point::new(INSET, INSET), Size::new(200.0, 200.0)),
            "the region's own box before any press"
        );
        h.event(secondary(PointerPhase::Down, 80.0, 90.0));
        assert_eq!(h.state.opens, vec![true]);
        assert!(h.state.open);
        // A zero-size anchor at the absolute press point.
        assert_eq!(
            h.anchor.rect(),
            Rect::from_origin_size(Point::new(80.0, 90.0), Size::ZERO)
        );
    }

    #[test]
    fn a_primary_press_passes_through_to_the_region_and_opens_nothing() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Down, 80.0, 90.0));
        h.event(pointer(PointerPhase::Up, 80.0, 90.0));
        assert_eq!(h.state.opens, Vec::<bool>::new());
        assert!(!h.state.open);
    }

    #[test]
    fn a_press_outside_the_panel_closes_it_and_escape_does_too() {
        let mut h = Harness::new();
        h.event(secondary(PointerPhase::Down, 80.0, 90.0));
        h.event(pointer(PointerPhase::Down, 380.0, 580.0));
        assert_eq!(h.state.opens, vec![true, false]);
        assert!(!h.state.open);

        h.event(secondary(PointerPhase::Down, 80.0, 90.0));
        // The anchor moves on the paint after the press, and the host re-places
        // itself on the layout after that — one more pass settles it.
        h.pass();
        // One press inside the panel focuses the host, which is what Escape
        // travels down (the framework has no focus-on-appear hook).
        let panel = h.anchor.rect();
        h.event(pointer(
            PointerPhase::Down,
            panel.x0 + 20.0,
            panel.y0 + 12.0,
        ));
        h.event(pointer(PointerPhase::Up, panel.x0 + 20.0, panel.y0 + 12.0));
        h.event(escape());
        assert!(!h.state.opens.last().unwrap(), "the last report is a close");
    }

    #[test]
    fn a_kept_mounted_menu_paints_out_its_exit_and_consumes_nothing_meanwhile() {
        let mut h = Harness::kept_mounted();
        h.event(secondary(PointerPhase::Down, 80.0, 90.0));
        // The press point lands in the anchor on the next paint, and the host
        // re-places itself on the layout after that.
        h.pass();
        h.paint_at(RAMP_MS * 2.0);
        assert!(Harness::panel_painted(&h.paint_at(h.clock)));
        let row = Point::new(h.anchor.rect().x0 + 20.0, h.anchor.rect().y0 + 12.0);

        // A press outside closes it; the menu stays mounted and ramps out.
        h.event(pointer(PointerPhase::Down, 380.0, 580.0));
        assert_eq!(h.state.opens, vec![true, false], "reported at once");
        assert!(!h.state.open);

        let start = h.clock;
        let mid = h.paint_at(start + RAMP_MS / 2.0);
        assert!(Harness::panel_painted(&mid), "still on screen");
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);

        let outcome = h
            .root
            .event(&mut h.state, &pointer(PointerPhase::Down, row.x, row.y));
        assert!(
            outcome.handled,
            "a closing menu swallows a press that lands on it"
        );
        assert_eq!(h.state.selected, Vec::<usize>::new());

        assert!(
            !Harness::panel_painted(&h.paint_at(start + RAMP_MS * 2.0)),
            "gone at settle"
        );
    }

    #[test]
    fn the_menu_reports_the_same_depth_first_indices_as_the_dropdown() {
        let mut h = Harness::new();
        h.event(secondary(PointerPhase::Down, 80.0, 90.0));
        h.pass();
        let panel = h.anchor.rect();
        // The first row sits just inside the panel's `p-1`.
        let row = Point::new(panel.x0 + 20.0, panel.y0 + 12.0);
        h.event(pointer(PointerPhase::Down, row.x, row.y));
        h.event(pointer(PointerPhase::Up, row.x, row.y));
        assert_eq!(h.state.selected, vec![0], "`Back`");
    }
}
