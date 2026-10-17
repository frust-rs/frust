//! Semantics id stability and accessibility action routing tests for RenderRoot.

use super::*;
use accesskit::{Action, NodeId, Role};

// Fixtures: an accessibility-visible button (`Role::Button`), a checkbox variant
// (`Role::CheckBox`), and a labelled leaf proving id stability across pod relocation.

/// A fire-on-up-inside button that also contributes a semantics node — the
/// end-to-end target for `perform_accessibility_action(Click)`.
struct A11yButtonWidget;

impl crate::widget::Widget for A11yButtonWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 20.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                PointerPhase::Up => {
                    let size = ctx.size();
                    let inside = p.position.x >= 0.0
                        && p.position.y >= 0.0
                        && p.position.x <= size.width
                        && p.position.y <= size.height;
                    if inside {
                        ctx.state_mut::<ClickState>().clicks += 1;
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                _ => {}
            }
        }
        EventResult::Ignored
    }
    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |n| n.set_label("Go"));
    }
}

struct A11yButtonView;

impl View<ClickState> for A11yButtonView {
    type Element = A11yButtonWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> A11yButtonWidget {
        A11yButtonWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _el: &mut A11yButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn a11y_button_logic(_state: &mut ClickState) -> A11yButtonView {
    A11yButtonView
}

fn button_node_id(update: &SemanticsUpdate, role: Role) -> NodeId {
    update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == role)
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("a {role:?} node is present"))
}

#[test]
fn semantics_ids_are_stable_across_frames() {
    let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut a11y_button_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let first = button_node_id(&root.semantics(), Role::Button);
    // Re-run rebuild+layout+semantics several times: the button keeps its id.
    for _ in 0..3 {
        root.rebuild(&mut a11y_button_logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        assert_eq!(
            button_node_id(&root.semantics(), Role::Button),
            first,
            "the same widget must keep its NodeId across frames"
        );
    }
    // The window root is the reserved constant id.
    assert_eq!(root.semantics().root, ROOT_NODE_ID);
}

#[test]
fn semantics_ids_survive_a_pod_relocation() {
    // A keyed reorder relocates the whole `ChildPod` (keeping its cached id); swap two
    // pods in place and assert each labelled node keeps its id.
    struct LabeledLeaf {
        label: &'static str,
    }
    impl crate::widget::Widget for LabeledLeaf {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(10.0, 10.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            let label = self.label;
            ctx.push_node(Role::Label, |n| n.set_label(label));
        }
    }

    let mut pods = vec![
        crate::widget::ChildPod::new(Box::new(LabeledLeaf { label: "A" })),
        crate::widget::ChildPod::new(Box::new(LabeledLeaf { label: "B" })),
    ];

    // Collect label -> id for a pod order, driving the pods directly (`SemanticsCtx`
    // is crate-private) through the allocation path `RenderRoot::semantics` uses.
    let collect = |pods: &[crate::widget::ChildPod]| {
        let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0), 2);
        for pod in pods {
            pod.semantics_child(&mut ctx);
        }
        let update = ctx.finish(ROOT_NODE_ID);
        update
            .nodes
            .iter()
            .filter(|(id, _)| *id != ROOT_NODE_ID)
            .map(|(id, n)| (n.label().unwrap().to_string(), *id))
            .collect::<Vec<_>>()
    };

    let before = collect(&pods);
    // Relocate: swap the pods (the pods themselves, with their cached ids,
    // move — mirroring the keyed reconciler's `take`-and-reorder).
    pods.swap(0, 1);
    let after = collect(&pods);

    for (label, id) in &before {
        let relocated = after.iter().find(|(l, _)| l == label).unwrap().1;
        assert_eq!(
            *id, relocated,
            "widget {label:?} must keep its NodeId across the reorder"
        );
    }
    // And the reorder actually changed positions (A now second).
    assert_eq!(after[0].0, "B");
    assert_eq!(after[1].0, "A");
}

#[test]
fn semantics_full_update_assembles_window_and_child() {
    let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut a11y_button_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let update = root.semantics();
    // Window root + the button.
    assert_eq!(update.nodes.len(), 2);
    assert_eq!(update.root, ROOT_NODE_ID);
    let root_node = update
        .nodes
        .iter()
        .find(|(id, _)| *id == update.root)
        .unwrap();
    assert_eq!(root_node.1.role(), Role::Window);
    let button = button_node_id(&update, Role::Button);
    assert_eq!(
        root_node.1.children(),
        &[button],
        "the button attaches under the window root"
    );
    // Nothing focused → the adapter-facing focus id defaults to the root.
    assert!(update.focus.is_none());
    assert_eq!(update.focus_id(), ROOT_NODE_ID);
}

#[test]
fn perform_click_action_activates_a_button() {
    let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut a11y_button_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let button = button_node_id(&root.semantics(), Role::Button);
    let outcome = root.perform_accessibility_action(&mut state, button, Action::Click);
    assert!(outcome.handled, "the synthesized Down+Up was handled");
    assert!(outcome.needs_redraw);
    assert_eq!(state.clicks, 1, "Click synthesized a real up-inside tap");

    // An unknown node id is a benign no-op.
    let outcome = root.perform_accessibility_action(&mut state, NodeId(999_999), Action::Click);
    assert_eq!(outcome, EventOutcome::default());
    assert_eq!(state.clicks, 1);

    // An unmodelled action is ignored.
    let outcome = root.perform_accessibility_action(&mut state, button, Action::ScrollDown);
    assert_eq!(outcome, EventOutcome::default());
    assert_eq!(state.clicks, 1);
}

#[test]
fn perform_click_action_toggles_a_checkbox() {
    // A checkbox-shaped widget reached through the same synthetic-pointer path:
    // Click drives any fire-on-up-inside control.
    struct CheckboxWidget;
    impl crate::widget::Widget for CheckboxWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(24.0, 24.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    PointerPhase::Up => {
                        let size = ctx.size();
                        if p.position.x >= 0.0
                            && p.position.y >= 0.0
                            && p.position.x <= size.width
                            && p.position.y <= size.height
                        {
                            ctx.state_mut::<ClickState>().clicks += 1;
                        }
                        return EventResult::Handled;
                    }
                    _ => {}
                }
            }
            EventResult::Ignored
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(Role::CheckBox, |n| n.set_label("Agree"));
        }
    }
    struct CheckboxView;
    impl View<ClickState> for CheckboxView {
        type Element = CheckboxWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> CheckboxWidget {
            CheckboxWidget
        }
        fn rebuild(
            &self,
            _p: &Self,
            _e: &mut CheckboxWidget,
            _c: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    let mut root: RenderRoot<ClickState, CheckboxView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut |_| CheckboxView, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let cb = button_node_id(&root.semantics(), Role::CheckBox);
    root.perform_accessibility_action(&mut state, cb, Action::Click);
    assert_eq!(state.clicks, 1, "Click toggled the checkbox once");
}

#[test]
fn perform_focus_action_claims_focus_and_clears_capture() {
    // Two focus-claiming buttons: a11y `Focus` on B must claim focus AND release the
    // capture the synthesized `Down` opened, or B swallows later events and A never gets a tap.

    #[derive(Default)]
    struct FocusState {
        a_press: u32,
        b_press: u32,
        b_move: u32,
    }

    #[derive(Clone, Copy)]
    enum Btn {
        A,
        B,
    }

    /// A button that opts into both recorded paths (capture + focus) on
    /// `Down` and fires its press only on `Up`-inside — never on `Cancel`.
    struct FocusButton {
        id: Btn,
    }
    impl crate::widget::Widget for FocusButton {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(40.0, 20.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            match p.phase {
                PointerPhase::Down => {
                    ctx.capture_pointer();
                    ctx.request_focus();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if let Btn::B = self.id {
                        ctx.state_mut::<FocusState>().b_move += 1;
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let size = ctx.size();
                    let inside = p.position.x >= 0.0
                        && p.position.y >= 0.0
                        && p.position.x <= size.width
                        && p.position.y <= size.height;
                    if inside {
                        match self.id {
                            Btn::A => ctx.state_mut::<FocusState>().a_press += 1,
                            Btn::B => ctx.state_mut::<FocusState>().b_press += 1,
                        }
                    }
                    EventResult::Handled
                }
                // A `Cancel` clears without firing on_press and never touches
                // state — the contract the Focus action's trailing Cancel rides.
                PointerPhase::Cancel => EventResult::Handled,
            }
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            let label = match self.id {
                Btn::A => "A",
                Btn::B => "B",
            };
            ctx.push_node(Role::Button, |n| n.set_label(label));
        }
    }

    /// A minimal two-child container mirroring `frust-widgets`'
    /// `route_event`: a captured gesture goes straight to the active child
    /// (auto-released on `Up`/`Cancel`), otherwise the event is hit-tested to
    /// the child under it.
    struct TwoButtons {
        a: crate::widget::ChildPod,
        b: crate::widget::ChildPod,
    }
    impl crate::widget::Widget for TwoButtons {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.a.layout_child(ctx, bc);
            self.a.set_origin(Point::new(0.0, 0.0));
            self.b.layout_child(ctx, bc);
            self.b.set_origin(Point::new(0.0, 30.0));
            bc.max()
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.a.paint_child(ctx, scene);
            self.b.paint_child(ctx, scene);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let releases = matches!(
                event,
                InputEvent::Pointer(p)
                    if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
            );
            // Capture fast-path: a recorded active child receives every event
            // until it releases on Up/Cancel, bypassing the hit test entirely.
            if self.a.is_active() {
                let r = self.a.event_child(ctx, event);
                if releases {
                    self.a.set_active(false);
                }
                return r;
            }
            if self.b.is_active() {
                let r = self.b.event_child(ctx, event);
                if releases {
                    self.b.set_active(false);
                }
                return r;
            }
            // Fresh event: route to the child under the point.
            let pos = event.position();
            if self.a.contains(pos) {
                return self.a.event_child(ctx, event);
            }
            if self.b.contains(pos) {
                return self.b.event_child(ctx, event);
            }
            EventResult::Ignored
        }
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            self.a.semantics_child(ctx);
            self.b.semantics_child(ctx);
        }
    }

    struct TwoButtonsView;
    impl View<FocusState> for TwoButtonsView {
        type Element = TwoButtons;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TwoButtons {
            TwoButtons {
                a: crate::widget::ChildPod::new(Box::new(FocusButton { id: Btn::A })),
                b: crate::widget::ChildPod::new(Box::new(FocusButton { id: Btn::B })),
            }
        }
        fn rebuild(&self, _p: &Self, _e: &mut TwoButtons, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    let mut root: RenderRoot<FocusState, TwoButtonsView> = RenderRoot::new();
    let mut state = FocusState::default();
    root.rebuild(&mut |_| TwoButtonsView, &mut state);
    root.layout(Size::new(200.0, 200.0));

    // B's semantics node (label "B") is the a11y Focus target.
    let b_id = root
        .semantics()
        .nodes
        .iter()
        .find(|(_, n)| n.label().is_some_and(|l| l == "B"))
        .map(|(id, _)| *id)
        .expect("button B contributes a semantics node");

    // A11y `Focus` on B: claims the focus session, fires no on_press, and —
    // crucially — leaves nothing captured (the Down+Cancel shape).
    root.perform_accessibility_action(&mut state, b_id, Action::Focus);
    assert!(root.is_focus_active(), "Focus opened the focus session");
    assert!(
        !root.is_pointer_captured(),
        "the trailing Cancel released the capture the Focus Down opened"
    );
    assert_eq!(
        state.b_press, 0,
        "Focus (Down+Cancel) must not fire B's on_press"
    );

    // B is not stuck-captured: a `Move` outside both buttons is ignored. Were
    // B still captured, the capture fast-path would route this to B regardless
    // of position (b_move would tick).
    let outside = InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Move,
        position: Point::new(100.0, 100.0),
        button: PointerButton::Primary,
    });
    root.event(&mut state, &outside);
    assert_eq!(state.b_move, 0, "no leaked capture: B saw no stray Move");

    // A real Down+Up on A activates A exactly once and never reaches B.
    let at_a = |phase| {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(20.0, 10.0),
            button: PointerButton::Primary,
        })
    };
    root.event(&mut state, &at_a(PointerPhase::Down));
    root.event(&mut state, &at_a(PointerPhase::Up));
    assert_eq!(state.a_press, 1, "A fired once from its own tap");
    assert_eq!(
        state.b_press, 0,
        "B never fired — its capture never leaked onto A's tap"
    );
}

#[test]
fn semantics_if_changed_gates_on_generation() {
    let mut root: RenderRoot<ClickState, A11yButtonView> = RenderRoot::new();
    let mut state = ClickState::default();
    // First build bumps the generation from 0.
    root.rebuild(&mut a11y_button_logic, &mut state);
    root.layout(Size::new(200.0, 200.0));

    let generation = root.semantics_generation();
    assert!(generation > 0);
    // A shell that already pushed `generation` sees no change.
    assert!(root.semantics_if_changed(generation).is_none());
    // A stale generation triggers a fresh pull.
    assert!(root.semantics_if_changed(generation - 1).is_some());

    // A theme swap marks the tree semantics-dirty.
    root.set_theme(Box::new(0u32));
    assert!(root.semantics_generation() > generation);
    assert!(root.semantics_if_changed(generation).is_some());
}
