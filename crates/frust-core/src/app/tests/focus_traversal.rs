//! Tab traversal at the root: walk order and wrap, the focus-path and shortcut steps it runs
//! after, modal confinement, floated surfaces, the release-then-grant session edge, and the
//! focus-ring policy.

use super::*;
use crate::overlay::{OverlayBand, OverlayKeys, OverlayPod};
use crate::shortcuts::{Chord, ShortcutDecl, ShortcutId, ShortcutScope};
use crate::widget::{ChildPod, Widget};

#[derive(Default)]
struct Log {
    /// `(name, has_focus, is_focused_for_ring)` for every logging widget on the latest paint.
    painted: Vec<(&'static str, bool, bool)>,
    /// Leaves that saw a plain `Key`.
    keys: Vec<&'static str>,
    /// Leaves that answered their own Tab shortcut.
    answered: Vec<&'static str>,
    /// Whether a Tab-declaring leaf answers its shortcut.
    shortcut_answers: bool,
}

type Shared = Rc<RefCell<Log>>;

#[derive(Clone, Copy, Default)]
struct Spec {
    focusable: bool,
    eats_tab: bool,
    declares_tab: bool,
    publishes_ime: bool,
}

const FOCUSABLE: Spec = Spec {
    focusable: true,
    eats_tab: false,
    declares_tab: false,
    publishes_ime: false,
};

struct Leaf {
    name: &'static str,
    spec: Spec,
    shortcut: ShortcutId,
    log: Shared,
}

impl Widget for Leaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 20.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let entry = (self.name, ctx.has_focus(), ctx.is_focused_for_ring());
        self.log.borrow_mut().painted.push(entry);
        if self.spec.publishes_ime && ctx.has_focus() {
            ctx.publish_ime_state(ImeState {
                active: true,
                ..Default::default()
            });
        }
        if self.spec.declares_tab {
            ctx.register_shortcut(ShortcutDecl {
                id: self.shortcut,
                chord: Chord::parse("Tab").unwrap(),
                label: self.name.to_string(),
                scope: ShortcutScope::AppGlobal,
            });
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                self.log.borrow_mut().keys.push(self.name);
                if key.key == Key::Named(NamedKey::Escape) {
                    ctx.release_focus();
                }
                let tab = key.key == Key::Named(NamedKey::Tab);
                if tab && self.spec.eats_tab {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Shortcut(id) if *id == self.shortcut => {
                let mut log = self.log.borrow_mut();
                if log.shortcut_answers {
                    log.answered.push(self.name);
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                EventResult::Handled
            }
            InputEvent::Scroll { .. } => {
                ctx.request_focus();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
    fn is_focusable(&self) -> bool {
        self.spec.focusable
    }
}

fn leaf_widget(name: &'static str, spec: Spec, log: &Shared) -> Box<dyn Widget> {
    Box::new(Leaf {
        name,
        spec,
        shortcut: ShortcutId::next(),
        log: Rc::clone(log),
    })
}

fn leaf(name: &'static str, spec: Spec, log: &Shared) -> ChildPod {
    ChildPod::new(leaf_widget(name, spec, log))
}

/// `widget` behind one more `Box<dyn Widget>` layer, so only the box's forwarding can report
/// it focusable or reach its children.
fn boxed(widget: Box<dyn Widget>) -> ChildPod {
    ChildPod::new(Box::new(widget))
}

/// A vertical stack routing like `frust-widgets`' helpers: broadcasts to every child, focus-routed
/// input to the child holding live focus, pointers by hit test. Logs its paint when focusable;
/// `deaf` drops shortcut broadcasts while still exposing its children to the focus walk.
struct Column {
    name: &'static str,
    focusable: bool,
    deaf: bool,
    children: Vec<ChildPod>,
    log: Shared,
}

impl Widget for Column {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let mut y = 0.0;
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
        }
        bc.constrain(Size::new(200.0, y))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.focusable {
            let entry = (self.name, ctx.has_focus(), ctx.is_focused_for_ring());
            self.log.borrow_mut().painted.push(entry);
        }
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            if self.deaf && matches!(event, InputEvent::Shortcut(_)) {
                return EventResult::Ignored;
            }
            for pod in &mut self.children {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if event.is_focus_routed() {
            return match self.children.iter_mut().find(|pod| pod.holds_live_focus()) {
                Some(pod) => pod.event_child(ctx, event),
                None => EventResult::Ignored,
            };
        }
        let position = event.position();
        match self.children.iter_mut().find(|pod| pod.contains(position)) {
            Some(pod) => pod.event_child(ctx, event),
            None => EventResult::Ignored,
        }
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        self.children.iter().for_each(visitor);
    }
    fn is_focusable(&self) -> bool {
        self.focusable
    }
}

fn column(name: &'static str, children: Vec<ChildPod>, log: &Shared) -> Column {
    Column {
        name,
        focusable: false,
        deaf: false,
        children,
        log: Rc::clone(log),
    }
}

/// Floats one pod; forwards its overlay events and, while the pod holds focus, focus-routed
/// input. `visits_pod` exposes the floated pod to `visit_children` (an inspectable owner).
struct FloatHost {
    key: OverlayKey,
    input: OverlayInput,
    keys: OverlayKeys,
    visits_pod: bool,
    pod: OverlayPod,
}

impl Widget for FloatHost {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let window = BoxConstraints::loose(ctx.window_size());
        self.pod.borrow_mut().layout_child(ctx, &window);
        bc.constrain(Size::new(40.0, 20.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.register_overlay(OverlayEntry {
            key: self.key,
            band: OverlayBand::Floating,
            input: self.input,
            outside_tap: OutsideTap::Ignore,
            keys: self.keys,
            window_rect: Rect::new(100.0, 100.0, 200.0, 200.0),
            pod: Rc::clone(&self.pod),
            insets: ctx.window_insets(),
        });
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let mut pod = self.pod.borrow_mut();
        match event {
            InputEvent::Overlay(overlay) if overlay.key == self.key => {
                let local = match &overlay.kind {
                    OverlayEventKind::Shortcut(id) => InputEvent::Shortcut(*id),
                    OverlayEventKind::Key(key) => InputEvent::Key(key.clone()),
                    _ => return EventResult::Ignored,
                };
                pod.event_child(ctx, &local);
                EventResult::Ignored
            }
            event if event.is_focus_routed() && pod.holds_live_focus() => {
                pod.event_child(ctx, event)
            }
            _ => EventResult::Ignored,
        }
    }
    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        if self.visits_pod {
            visitor(&self.pod.borrow());
        }
    }
}

fn float_host(input: OverlayInput, keys: OverlayKeys, visits_pod: bool, pod: Column) -> ChildPod {
    ChildPod::new(Box::new(FloatHost {
        key: OverlayKey::next(),
        input,
        keys,
        visits_pod,
        pod: Rc::new(RefCell::new(ChildPod::new(Box::new(pod)))),
    }))
}

struct TreeView {
    make: fn(&Shared) -> Column,
    log: Shared,
}

impl View<()> for TreeView {
    type Element = Column;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Column {
        (self.make)(&self.log)
    }
    fn rebuild(&self, _prev: &Self, _element: &mut Column, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

struct Harness {
    root: RenderRoot<(), TreeView>,
    make: fn(&Shared) -> Column,
    log: Shared,
}

impl Harness {
    fn new(make: fn(&Shared) -> Column) -> Self {
        let log = Shared::default();
        log.borrow_mut().shortcut_answers = true;
        let mut harness = Self {
            root: RenderRoot::new(),
            make,
            log,
        };
        harness.frame();
        harness
    }

    fn frame(&mut self) {
        self.log.borrow_mut().painted.clear();
        let (make, log) = (self.make, Rc::clone(&self.log));
        let mut logic = move |_: &mut ()| TreeView {
            make,
            log: Rc::clone(&log),
        };
        self.root.rebuild(&mut logic, &mut ());
        self.root.layout(Size::new(200.0, 200.0));
        self.root
            .paint(&mut RecordingScene::default(), FrameTime::ZERO);
    }

    /// Dispatch `event`, then run a frame so the paint log reflects it.
    fn send(&mut self, event: &InputEvent) -> EventOutcome {
        let outcome = self.root.event(&mut (), event);
        self.frame();
        outcome
    }

    fn tab(&mut self) -> EventOutcome {
        self.send(&tab_key(false))
    }

    fn shift_tab(&mut self) -> EventOutcome {
        self.send(&tab_key(true))
    }

    /// The deepest logging widget that painted focused (paint order is pre-order).
    fn focused(&self) -> Option<&'static str> {
        let log = self.log.borrow();
        log.painted.iter().rev().find(|e| e.1).map(|e| e.0)
    }

    fn ringed(&self) -> Vec<&'static str> {
        let log = self.log.borrow();
        log.painted.iter().filter(|e| e.2).map(|e| e.0).collect()
    }
}

fn tab_key(shift: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers {
            shift,
            ..Modifiers::default()
        },
        repeat: false,
    })
}

/// `a`, `b` (not focusable), `col` (focusable: `c`, boxed `d`), boxed column (`e`).
fn nested(log: &Shared) -> Column {
    let mut col = column("col", vec![leaf("c", FOCUSABLE, log)], log);
    col.focusable = true;
    col.children.push(boxed(leaf_widget("d", FOCUSABLE, log)));
    let tail: Box<dyn Widget> = Box::new(column("tail", vec![leaf("e", FOCUSABLE, log)], log));
    let children = vec![
        leaf("a", FOCUSABLE, log),
        leaf("b", Spec::default(), log),
        ChildPod::new(Box::new(col)),
        boxed(tail),
    ];
    column("root", children, log)
}

#[test]
fn tab_walks_focusables_in_pre_order_through_boxes_and_wraps() {
    let mut h = Harness::new(nested);
    assert_eq!(h.focused(), None);
    let mut order = Vec::new();
    for _ in 0..6 {
        assert!(h.tab().handled, "a traversal answers the key");
        order.push(h.focused().unwrap());
    }
    assert_eq!(order, ["a", "col", "c", "d", "e", "a"]);
}

#[test]
fn shift_tab_walks_backwards_and_starts_from_the_last_stop() {
    let mut h = Harness::new(nested);
    let mut order = Vec::new();
    for _ in 0..6 {
        h.shift_tab();
        order.push(h.focused().unwrap());
    }
    assert_eq!(order, ["e", "d", "c", "col", "a", "e"]);
}

#[test]
fn modified_tab_and_a_tree_without_focusables_leave_focus_alone() {
    let mut h = Harness::new(|log| column("root", vec![leaf("a", Spec::default(), log)], log));
    assert!(!h.tab().handled);
    assert!(!h.root.is_focus_active());

    let mut h = Harness::new(nested);
    let ctrl_tab = InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    assert!(!h.send(&ctrl_tab).handled);
    assert_eq!(h.focused(), None);
}

#[test]
fn a_focused_widget_consuming_tab_blocks_traversal() {
    const EATER: Spec = Spec {
        eats_tab: true,
        ..FOCUSABLE
    };
    let mut h = Harness::new(|log| {
        let children = vec![leaf("a", EATER, log), leaf("b", FOCUSABLE, log)];
        column("root", children, log)
    });
    h.tab();
    assert_eq!(h.focused(), Some("a"));
    assert!(h.tab().handled);
    assert_eq!(h.focused(), Some("a"), "the field kept its Tab");
    assert_eq!(h.log.borrow().keys, ["a"]);
}

#[test]
fn a_tab_shortcut_declaration_runs_before_traversal() {
    const DECLARER: Spec = Spec {
        declares_tab: true,
        ..FOCUSABLE
    };
    let mut h = Harness::new(|log| {
        let children = vec![
            leaf(
                "s",
                Spec {
                    focusable: false,
                    ..DECLARER
                },
                log,
            ),
            leaf("a", FOCUSABLE, log),
        ];
        column("root", children, log)
    });
    assert!(h.tab().handled);
    assert_eq!(h.log.borrow().answered, ["s"]);
    assert_eq!(h.focused(), None, "an answered shortcut ends the key");

    // A declarer that declines hands the key on to traversal.
    h.log.borrow_mut().shortcut_answers = false;
    h.tab();
    assert_eq!(h.focused(), Some("a"));
}

#[test]
fn a_modal_pod_confines_traversal_to_its_own_subtree() {
    let mut h = Harness::new(|log| {
        let pod = column(
            "modal",
            vec![leaf("m1", FOCUSABLE, log), leaf("m2", FOCUSABLE, log)],
            log,
        );
        let host = float_host(OverlayInput::Interactive, OverlayKeys::Modal, false, pod);
        let children = vec![leaf("a", FOCUSABLE, log), host, leaf("b", FOCUSABLE, log)];
        column("root", children, log)
    });
    let mut order = Vec::new();
    for _ in 0..3 {
        h.tab();
        order.push(h.focused().unwrap());
    }
    assert_eq!(order, ["m1", "m2", "m1"]);
    h.shift_tab();
    assert_eq!(h.focused(), Some("m2"));
}

#[test]
fn traversal_skips_transparent_pods_and_reaches_interactive_ones_through_their_owner() {
    let mut h = Harness::new(|log| {
        let ghost = column("ghost", vec![leaf("t", FOCUSABLE, log)], log);
        let popover = column("popover", vec![leaf("p", FOCUSABLE, log)], log);
        let children = vec![
            leaf("a", FOCUSABLE, log),
            float_host(OverlayInput::Transparent, OverlayKeys::Pass, true, ghost),
            float_host(OverlayInput::Interactive, OverlayKeys::Pass, true, popover),
        ];
        column("root", children, log)
    });
    let mut order = Vec::new();
    for _ in 0..3 {
        h.tab();
        order.push(h.focused().unwrap());
    }
    assert_eq!(order, ["a", "p", "a"]);
}

#[test]
fn a_stop_the_grant_cannot_reach_is_skipped_for_the_next() {
    let mut h = Harness::new(|log| {
        let mut deaf = column("deaf", vec![leaf("x", FOCUSABLE, log)], log);
        deaf.deaf = true;
        let children = vec![
            ChildPod::new(Box::new(deaf)),
            leaf("a", FOCUSABLE, log),
            leaf("b", FOCUSABLE, log),
        ];
        column("root", children, log)
    });
    let mut order = Vec::new();
    for _ in 0..3 {
        h.tab();
        order.push(h.focused().unwrap());
    }
    assert_eq!(order, ["a", "b", "a"]);
    h.shift_tab();
    assert_eq!(h.focused(), Some("b"));
}

#[test]
fn the_old_session_is_released_before_the_grant_claims_a_fresh_one() {
    const FIELD: Spec = Spec {
        publishes_ime: true,
        ..FOCUSABLE
    };
    let mut h = Harness::new(|log| {
        let children = vec![leaf("a", FIELD, log), leaf("b", FIELD, log)];
        column("root", children, log)
    });
    h.tab();
    assert!(h.root.ime_state().is_some(), "a publishes while focused");
    let (epoch, generation) = (h.root.focus_epoch(), h.root.focus_ime_generation());

    assert!(h.root.event(&mut (), &tab_key(false)).handled);
    assert!(h.root.is_focus_active());
    assert_eq!(
        h.root.ime_state(),
        None,
        "a's surface ended with its session"
    );
    assert_eq!(h.root.focus_epoch(), epoch + 2, "a release, then a claim");
    assert_eq!(h.root.focus_ime_generation(), generation + 2);
    h.frame();
    assert_eq!(h.focused(), Some("b"));
    assert!(h.root.ime_state().is_some(), "b publishes its own surface");
}

fn pair(log: &Shared) -> Column {
    column(
        "root",
        vec![leaf("a", FOCUSABLE, log), leaf("b", FOCUSABLE, log)],
        log,
    )
}

#[test]
fn a_focusable_root_widget_is_the_first_stop() {
    let mut h = Harness::new(|log| Column {
        focusable: true,
        ..pair(log)
    });
    let mut order = Vec::new();
    for _ in 0..4 {
        assert!(h.tab().handled);
        order.push(h.focused().unwrap());
    }
    assert_eq!(order, ["root", "a", "b", "root"]);
}

/// `a`, then `x` inside a column that drops every grant.
fn deaf_tail(log: &Shared) -> Column {
    let mut deaf = column("deaf", vec![leaf("x", FOCUSABLE, log)], log);
    deaf.deaf = true;
    let children = vec![leaf("a", FOCUSABLE, log), ChildPod::new(Box::new(deaf))];
    column("root", children, log)
}

#[test]
fn a_walk_where_no_grant_lands_keeps_the_session_and_declines_the_key() {
    let mut h = Harness::new(deaf_tail);
    assert!(h.tab().handled);
    assert_eq!(h.focused(), Some("a"));
    let (epoch, generation) = (h.root.focus_epoch(), h.root.focus_ime_generation());

    assert!(!h.tab().handled);
    assert!(h.root.is_focus_active());
    assert_eq!(h.root.focus_epoch(), epoch);
    assert_eq!(h.root.focus_ime_generation(), generation);
    assert_eq!(h.focused(), Some("a"));
    assert_eq!(h.ringed(), ["a"]);

    let mut h = Harness::new(|log| {
        let mut deaf = column("deaf", vec![leaf("x", FOCUSABLE, log)], log);
        deaf.deaf = true;
        column("root", vec![ChildPod::new(Box::new(deaf))], log)
    });
    assert!(!h.tab().handled);
    assert!(!h.root.is_focus_active());
}

#[test]
fn a_claim_outside_traversal_or_a_release_drops_the_keyboard_flag() {
    let mut h = Harness::new(pair);
    h.tab();
    assert_eq!(h.ringed(), ["a"]);
    let scroll = InputEvent::Scroll {
        position: Point::new(5.0, 25.0),
        delta: ScrollDelta::Lines(0.0, 1.0),
    };
    h.send(&scroll);
    assert_eq!(h.focused(), Some("b"));
    assert!(
        h.ringed().is_empty(),
        "a claim with no `Down` shows no ring"
    );

    h.tab();
    assert_eq!(h.ringed(), ["a"]);
    let escape = InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Escape),
        modifiers: Modifiers::default(),
        repeat: false,
    });
    h.send(&escape);
    assert!(!h.root.is_focus_active());
    assert!(!h.root.focus_ring_visible(), "the release cleared it");
}

#[test]
fn with_tab_traversal_off_a_declined_tab_stays_unhandled() {
    let mut h = Harness::new(pair);
    assert!(h.root.tab_traversal());
    h.root.set_tab_traversal(false);
    assert!(!h.tab().handled);
    assert!(!h.shift_tab().handled);
    assert_eq!(h.focused(), None);

    h.root.set_tab_traversal(true);
    assert!(h.tab().handled);
    assert_eq!(h.focused(), Some("a"));
}

#[test]
fn keyboard_only_rings_traversal_focus_until_a_pointer_down() {
    let mut h = Harness::new(pair);
    assert_eq!(h.root.focus_ring_policy(), FocusRingPolicy::KeyboardOnly);
    h.tab();
    assert_eq!(h.ringed(), ["a"]);

    // b sits at y 20..40 and claims focus on its own `Down`.
    h.send(&pointer(PointerPhase::Down, 5.0, 25.0));
    assert_eq!(h.focused(), Some("b"));
    assert!(h.ringed().is_empty(), "a pointer claim shows no ring");

    // A Tab with focus already somewhere brings the ring back.
    h.tab();
    assert_eq!(h.ringed(), ["a"]);
}

#[test]
fn never_and_always_ignore_where_focus_came_from() {
    let mut h = Harness::new(pair);
    h.root.set_focus_ring_policy(FocusRingPolicy::Never);
    h.tab();
    assert_eq!(h.focused(), Some("a"));
    assert!(h.ringed().is_empty());

    h.root.set_focus_ring_policy(FocusRingPolicy::Always);
    h.send(&pointer(PointerPhase::Down, 5.0, 25.0));
    assert_eq!(h.ringed(), ["b"]);
}

#[test]
fn a_policy_change_repaints_a_retained_tree() {
    let mut h = Harness::new(pair);
    h.tab();
    let _ = h.root.take_change_flags();
    h.root.set_focus_ring_policy(FocusRingPolicy::Never);
    assert!(h.root.take_change_flags().contains(ChangeFlags::PAINT));
    h.frame();
    assert!(h.ringed().is_empty());
}
