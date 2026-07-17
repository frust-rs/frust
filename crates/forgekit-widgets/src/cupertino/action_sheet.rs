//! `CupertinoActionSheet` (Phase 6c, PLAN.md D3/D5, task 13): the iOS action
//! sheet — a bottom-anchored list of action rows plus a separate cancel block,
//! over a dimming scrim.
//!
//! It is pushed as a **transparent** navigator page (task 06) via the
//! [`show_action_sheet`] convenience, entering with the
//! [`PageTransition::SlideUp`](crate::PageTransition::SlideUp) preset — the
//! whole page (scrim included) translates up from the bottom, so the scrim is
//! **page-owned** and drawn here at a fixed alpha rather than driven by the
//! transition (see `crate::nav::transition`'s `SlideUp` docs). An action tap
//! pops carrying the action's index as a [`PopResult`]; the cancel row or a
//! scrim tap pops with an empty result (dismiss).
//!
//! Reuses [`super::alert_dialog`]'s [`CupertinoActionStyle`]/
//! [`CupertinoDialogAction`]/[`action`] vocabulary. Colors and the ~14pt corner
//! carry the same community-approximate flags documented there.
//!
//! # Safe area
//!
//! The sheet anchors to a fixed bottom margin. The home-indicator safe-area
//! inset a real iOS sheet respects is **shell-future work** (documented,
//! PLAN.md D5).
//!
//! # Keyboard operability (limitation)
//!
//! **There is no keyboard focus or Escape-to-dismiss for this action sheet
//! today.** `Key` events route only down the recorded focus path (the
//! [`InputEvent::is_focus_routed`] dispatch gate — see
//! `docs/ARCHITECTURE.md`'s Event pipeline), and this widget never calls
//! `request_focus`, so there is no focused chain for an Escape key to travel
//! and no way to wire dismiss-on-Escape without the focus-routing work that is
//! 6d scope. Dismissal is pointer-only: a scrim tap, the cancel row, or an
//! action row. Its semantics node is a [`Role::Menu`] container with **no**
//! accesskit modal flag, so there is no modal-audience concern to reconcile;
//! keyboard operability + platform `accesskit_*` adapter wiring is 6d scope.

use forgekit_core::accesskit::Role;
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, Size};
use peniko::Color;

use super::alert_dialog::{CupertinoActionStyle, CupertinoDialogAction, action, action_label_view};
use crate::nav::navigator::{NavigatorController, PopResult};

/// Horizontal margin from the window edges to the sheet panels, in logical px.
///
/// **Community-approximate**: ~8pt matches the inset iOS action sheets sit at;
/// no published margin spec exists.
const SIDE_MARGIN: f64 = 8.0;
/// Bottom margin from the window's bottom edge to the cancel block, px
/// (**community-approximate**; the home-indicator safe area is shell-future
/// work — see the [module docs](self)).
const BOTTOM_MARGIN: f64 = 8.0;
/// Gap between the main action panel and the separate cancel block, px.
const BLOCK_GAP: f64 = 8.0;
/// Height of each action/cancel row, in logical px.
///
/// **Community-approximate**: ~56pt matches the taller iOS action-sheet row
/// (vs. an alert's ~44pt); no published spec exists.
const ROW_H: f64 = 56.0;
/// Panel corner radius, logical px (**community-approximate**, ~13-14pt — see
/// [`super::alert_dialog`]'s identical flag).
const PANEL_RADIUS: f64 = 14.0;

/// Unthemed fallback panel fill (a theme resolves this from
/// `colors.surface_container_high`).
const PANEL_FILL: Color = Color::from_rgb8(0xF2, 0xF2, 0xF7);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`).
const SEPARATOR: Color = Color::from_rgb8(0xC6, 0xC6, 0xC8);
/// The scrim alpha the modal dims the page below with (see
/// [`super::alert_dialog`]'s identical flag).
const SCRIM_ALPHA: f32 = 0.2;
/// Hairline stroke width, logical px.
const HAIRLINE_W: f64 = 1.0;

/// Push a Cupertino action sheet onto `controller`'s stack. `on_result`
/// receives the tapped action's index (`usize`) as a [`PopResult`], or an empty
/// result when the cancel row or scrim was tapped.
pub fn show_action_sheet<State: 'static>(
    controller: &NavigatorController<State>,
    actions: Vec<CupertinoDialogAction>,
    cancel: Option<String>,
    on_result: impl Fn(&mut State, PopResult) + 'static,
) {
    let controller_for_builder = controller.clone();
    controller.push_transparent_for_result(
        move || {
            forgekit_core::any::<State, _>(CupertinoActionSheetView {
                actions: actions.clone(),
                cancel: cancel.clone(),
                controller: controller_for_builder.clone(),
            })
        },
        crate::TransitionSpec::duration(crate::PageTransition::SlideUp),
        on_result,
    );
}

/// A declarative iOS action sheet page. Usually created via
/// [`show_action_sheet`]; exposed so it can also be tested/embedded directly.
pub struct CupertinoActionSheetView<State: 'static> {
    pub actions: Vec<CupertinoDialogAction>,
    pub cancel: Option<String>,
    pub controller: NavigatorController<State>,
}

/// The retained widget for a [`CupertinoActionSheetView`].
pub struct CupertinoActionSheetWidget<State: 'static> {
    actions: Vec<ChildPod>,
    cancel: Option<ChildPod>,
    controller: NavigatorController<State>,
    /// Main action panel rect in local coordinates (computed in layout).
    main_rect: Rect,
    /// Each action row's rect in local coordinates.
    action_rects: Vec<Rect>,
    /// The cancel block rect in local coordinates (if a cancel row exists).
    cancel_rect: Option<Rect>,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` (fire-on-up-inside).
    captured: bool,
}

/// Build the cancel row's type-erased child view (bold systemBlue, centered).
fn cancel_view<State: 'static>(label: String) -> AnyView<State> {
    action_label_view::<State>(&action(label).style(CupertinoActionStyle::Cancel))
}

impl<State: 'static> View<State> for CupertinoActionSheetView<State> {
    type Element = CupertinoActionSheetWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoActionSheetWidget<State> {
        let actions = self
            .actions
            .iter()
            .map(|a| crate::build_child(&action_label_view::<State>(a), ctx))
            .collect();
        let cancel = self
            .cancel
            .as_ref()
            .map(|c| crate::build_child(&cancel_view::<State>(c.clone()), ctx));
        CupertinoActionSheetWidget {
            actions,
            cancel,
            controller: self.controller.clone(),
            main_rect: Rect::ZERO,
            action_rects: Vec::new(),
            cancel_rect: None,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoActionSheetWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.controller = self.controller.clone();
        let common = prev.actions.len().min(self.actions.len());
        for i in 0..common {
            flags |= crate::rebuild_child(
                &action_label_view::<State>(&prev.actions[i]),
                &action_label_view::<State>(&self.actions[i]),
                &mut element.actions[i],
                ctx,
            );
        }
        if let (Some(pc), Some(nc), Some(elem_c)) =
            (&prev.cancel, &self.cancel, element.cancel.as_mut())
        {
            flags |= crate::rebuild_child(
                &cancel_view::<State>(pc.clone()),
                &cancel_view::<State>(nc.clone()),
                elem_c,
                ctx,
            );
        }
        flags
    }

    fn teardown(&self, element: &mut CupertinoActionSheetWidget<State>, ctx: &mut BuildCtx<'_>) {
        for (a, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            crate::teardown_child(&action_label_view::<State>(a), pod, ctx);
        }
        if let (Some(c), Some(elem_c)) = (&self.cancel, element.cancel.as_mut()) {
            crate::teardown_child(&cancel_view::<State>(c.clone()), elem_c, ctx);
        }
    }
}

impl<State: 'static> Widget for CupertinoActionSheetWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let full = bc.max();
        let panel_w = (full.width - 2.0 * SIDE_MARGIN).max(0.0);
        let panel_x = SIDE_MARGIN;
        let row_bc = BoxConstraints::loose(Size::new(panel_w, ROW_H));

        for pod in &mut self.actions {
            pod.layout_child(ctx, &row_bc);
        }
        if let Some(c) = self.cancel.as_mut() {
            c.layout_child(ctx, &row_bc);
        }

        // Cancel block hugs the bottom; the main panel sits above it.
        let (cancel_top, main_bottom) = if self.cancel.is_some() {
            let cancel_bottom = full.height - BOTTOM_MARGIN;
            let cancel_top = cancel_bottom - ROW_H;
            self.cancel_rect = Some(Rect::new(
                panel_x,
                cancel_top,
                panel_x + panel_w,
                cancel_bottom,
            ));
            (cancel_top, cancel_top - BLOCK_GAP)
        } else {
            self.cancel_rect = None;
            (0.0, full.height - BOTTOM_MARGIN)
        };
        let _ = cancel_top;

        let main_h = self.actions.len() as f64 * ROW_H;
        let main_top = main_bottom - main_h;
        self.main_rect = Rect::new(panel_x, main_top, panel_x + panel_w, main_bottom);

        // Action rows stacked top-to-bottom in the main panel; center labels.
        self.action_rects.clear();
        for (i, pod) in self.actions.iter_mut().enumerate() {
            let row_y = main_top + i as f64 * ROW_H;
            let rect = Rect::new(panel_x, row_y, panel_x + panel_w, row_y + ROW_H);
            let ls = pod.size();
            pod.set_origin(Point::new(
                panel_x + (panel_w - ls.width) / 2.0,
                row_y + (ROW_H - ls.height) / 2.0,
            ));
            self.action_rects.push(rect);
        }

        // Center the cancel label in its block.
        if let (Some(c), Some(rect)) = (self.cancel.as_mut(), self.cancel_rect) {
            let ls = c.size();
            c.set_origin(Point::new(
                panel_x + (panel_w - ls.width) / 2.0,
                rect.y0 + (ROW_H - ls.height) / 2.0,
            ));
        }

        bc.constrain(full)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let panel_fill = theme
            .map(|t| t.scheme().surface_container_high)
            .unwrap_or(PANEL_FILL);
        let separator = theme
            .map(|t| t.scheme().outline_variant)
            .unwrap_or(SEPARATOR);
        let scrim_base = theme.map(|t| t.scheme().scrim).unwrap_or(Color::BLACK);

        let origin = ctx.origin();
        let size = ctx.size();
        // Page-owned scrim (SlideUp does not fade layers — see the module docs).
        let scrim = {
            let c = scrim_base.components;
            Color::new([c[0], c[1], c[2], SCRIM_ALPHA])
        };
        scene.fill_rect(origin, size, scrim);

        // Main action panel.
        let main_origin = Point::new(origin.x + self.main_rect.x0, origin.y + self.main_rect.y0);
        scene.fill_rounded_rect(
            main_origin,
            Size::new(self.main_rect.width(), self.main_rect.height()),
            PANEL_RADIUS,
            panel_fill,
        );
        // Hairlines between action rows (skip the first row's top).
        for rect in self.action_rects.iter().skip(1) {
            let y = origin.y + rect.y0 + HAIRLINE_W / 2.0;
            scene.stroke_line(
                Point::new(origin.x + rect.x0, y),
                Point::new(origin.x + rect.x1, y),
                HAIRLINE_W,
                separator,
            );
        }

        // Separate cancel block.
        if let Some(rect) = self.cancel_rect {
            let c_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            scene.fill_rounded_rect(
                c_origin,
                Size::new(rect.width(), rect.height()),
                PANEL_RADIUS,
                panel_fill,
            );
        }

        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
        if let Some(c) = self.cancel.as_mut() {
            c.paint_child(ctx, scene);
        }
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
                self.captured = false;
                let pos = p.position;
                if let Some(idx) = self.action_rects.iter().position(|r| r.contains(pos)) {
                    self.controller.pop_with_result(PopResult::of(idx));
                    ctx.request_redraw();
                } else if self.cancel_rect.map(|r| r.contains(pos)).unwrap_or(false) {
                    // Cancel row → dismiss (empty result).
                    self.controller.pop();
                    ctx.request_redraw();
                } else if !self.main_rect.contains(pos) {
                    // Scrim tap → dismiss.
                    self.controller.pop();
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let actions = &self.actions;
        let cancel = &self.cancel;
        // An action sheet reads to a screen reader like a menu of choices.
        ctx.push_container(
            Role::Menu,
            |_| {},
            |ctx| {
                for pod in actions {
                    pod.semantics_child(ctx);
                }
                if let Some(c) = cancel {
                    c.semantics_child(ctx);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorView, navigator};
    use forgekit_core::{BuildCtx, FrameTime, PointerButton, PointerEvent, RenderRoot, any};
    use forgekit_text::TextContext;
    use std::any::Any;

    const WINDOW: Size = Size::new(400.0, 800.0);

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    fn sheet_view() -> CupertinoActionSheetView<()> {
        CupertinoActionSheetView {
            actions: vec![
                action("Save"),
                action("Delete").style(CupertinoActionStyle::Destructive),
            ],
            cancel: Some("Cancel".into()),
            controller: NavigatorController::new(),
        }
    }

    fn build(view: &CupertinoActionSheetView<()>) -> CupertinoActionSheetWidget<()> {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CupertinoActionSheetWidget<()>, window: Size) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, &BoxConstraints::tight(window));
    }

    #[test]
    fn panels_anchor_to_the_bottom_with_cancel_below_actions() {
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        assert_eq!(w.action_rects.len(), 2);
        let cancel = w.cancel_rect.expect("cancel block present");
        // Cancel sits below the main panel.
        assert!(cancel.y0 > w.main_rect.y1 - 1e-6);
        // Cancel bottom respects the bottom margin.
        assert!((cancel.y1 - (WINDOW.height - BOTTOM_MARGIN)).abs() < 1e-6);
        // The whole sheet hugs the bottom half of the window.
        assert!(w.main_rect.y0 > WINDOW.height / 2.0);
    }

    #[test]
    fn panel_width_respects_side_margins() {
        let mut w = build(&sheet_view());
        layout(&mut w, WINDOW);
        assert!((w.main_rect.width() - (WINDOW.width - 2.0 * SIDE_MARGIN)).abs() < 1e-6);
        assert_eq!(w.main_rect.x0, SIDE_MARGIN);
    }

    // --- Modal wiring through a real navigator (mirrors alert_dialog's
    //     harness), settling the entering SlideUp transition first. ---

    #[derive(Default)]
    struct SheetState {
        calls: u32,
        received: Option<usize>,
    }

    struct Harness {
        root: RenderRoot<SheetState, NavigatorView<SheetState>>,
        controller: NavigatorController<SheetState>,
        state: SheetState,
        tcx: TextContext,
        probe: CupertinoActionSheetWidget<()>,
    }

    impl Harness {
        fn new() -> Self {
            let controller: NavigatorController<SheetState> = NavigatorController::new();
            let root: RenderRoot<SheetState, NavigatorView<SheetState>> = RenderRoot::new();
            let mut probe = build(&sheet_view());
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
            probe.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
            Harness {
                root,
                controller,
                state: SheetState::default(),
                tcx: TextContext::new(),
                probe,
            }
        }

        fn app(&self) -> impl FnMut(&mut SheetState) -> NavigatorView<SheetState> + use<> {
            let ctrl = self.controller.clone();
            move |_: &mut SheetState| navigator(&ctrl, || any(crate::text::text("base").size(17.0)))
        }

        fn rebuild_layout(&mut self) {
            let mut app = self.app();
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn show(&mut self) {
            self.rebuild_layout();
            show_action_sheet(
                &self.controller,
                vec![
                    action("Save"),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                Some("Cancel".into()),
                |s: &mut SheetState, r: PopResult| {
                    s.calls += 1;
                    if let Some(i) = r.take::<usize>() {
                        s.received = Some(i);
                    }
                },
            );
            self.rebuild_layout();
        }

        fn settle(&mut self) {
            let mut scene = crate::test_support::RecordingScene::default();
            self.root.paint(&mut scene, ft_secs(0.0));
            self.root.paint(&mut scene, ft_secs(1.0));
            self.rebuild_layout();
        }

        fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            })
        }

        fn tap(&mut self, x: f64, y: f64) {
            self.root
                .event(&mut self.state, &Self::ev(PointerPhase::Down, x, y));
            self.root
                .event(&mut self.state, &Self::ev(PointerPhase::Up, x, y));
        }

        fn drain(&mut self) {
            self.rebuild_layout();
            self.root
                .event(&mut self.state, &Self::ev(PointerPhase::Move, 1.0, 1.0));
        }

        fn action_center(&self, idx: usize) -> (f64, f64) {
            let r = self.probe.action_rects[idx];
            ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
        }

        fn cancel_center(&self) -> (f64, f64) {
            let r = self.probe.cancel_rect.unwrap();
            ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
        }
    }

    #[test]
    fn action_tap_pops_with_its_index() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        let (cx, cy) = h.action_center(1);
        h.tap(cx, cy);
        h.drain();
        assert_eq!(h.state.calls, 1);
        assert_eq!(h.state.received, Some(1));
    }

    #[test]
    fn cancel_tap_dismisses() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        let (cx, cy) = h.cancel_center();
        h.tap(cx, cy);
        h.drain();
        assert_eq!(h.state.calls, 1, "cancel pops the sheet");
        assert_eq!(h.state.received, None, "cancel carries no action index");
    }

    #[test]
    fn scrim_tap_dismisses() {
        let mut h = Harness::new();
        h.show();
        h.settle();
        // Top-left corner — above the bottom-anchored sheet.
        h.tap(5.0, 5.0);
        h.drain();
        assert_eq!(h.state.calls, 1);
        assert_eq!(h.state.received, None);
    }

    #[test]
    fn semantics_is_a_menu_container() {
        fn logic(_s: &mut ()) -> CupertinoActionSheetView<()> {
            sheet_view()
        }
        let mut root: RenderRoot<(), CupertinoActionSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Menu),
            "a Menu container node is contributed"
        );
    }
}
