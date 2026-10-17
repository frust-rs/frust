use super::*;

/// What every probe saw: which probe, which contact, which phase.
#[derive(Default)]
struct Log {
    seen: Vec<(char, PointerId, PointerPhase)>,
}

/// A leaf that logs every pointer event it receives with the contact id its
/// context reports, and on a `Down` captures (optionally opting into the
/// gesture's other contacts) and claims focus.
struct Probe {
    tag: char,
    opt_in: bool,
}
impl crate::widget::Widget for Probe {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(40.0, 20.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let id = ctx.pointer_id();
        ctx.state_mut::<Log>().seen.push((self.tag, id, p.phase));
        if p.phase == PointerPhase::Down {
            ctx.capture_pointer();
            if self.opt_in {
                ctx.capture_contacts();
            }
            ctx.request_focus();
        }
        EventResult::Handled
    }
}

/// Two probes stacked vertically (A at y 0..20, B at y 30..50) behind the
/// standard recorded-path routing: a captured gesture goes straight to the
/// active child and releases it on `Up`/`Cancel`; anything else is
/// hit-tested, and a `Down` that hits nothing blurs both.
struct Pair {
    a: crate::widget::ChildPod,
    b: crate::widget::ChildPod,
}
impl crate::widget::Widget for Pair {
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
            InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
        );
        for pod in [&mut self.a, &mut self.b] {
            if pod.is_active() {
                let result = pod.event_child(ctx, event);
                if releases {
                    pod.set_active(false);
                }
                return result;
            }
        }
        let pos = event.position();
        let down = matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down);
        for pod in [&mut self.a, &mut self.b] {
            if pod.contains(pos) {
                return pod.event_child(ctx, event);
            }
        }
        if down {
            self.a.set_focused(false);
            self.b.set_focused(false);
        }
        EventResult::Ignored
    }
}

struct PairView {
    opt_in: bool,
}
impl View<Log> for PairView {
    type Element = Pair;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Pair {
        Pair {
            a: crate::widget::ChildPod::new(Box::new(Probe {
                tag: 'A',
                opt_in: self.opt_in,
            })),
            b: crate::widget::ChildPod::new(Box::new(Probe {
                tag: 'B',
                opt_in: self.opt_in,
            })),
        }
    }
    fn rebuild(&self, _p: &Self, _e: &mut Pair, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn root_with(opt_in: bool) -> (RenderRoot<Log, PairView>, Log) {
    let mut root: RenderRoot<Log, PairView> = RenderRoot::new();
    let mut log = Log::default();
    root.rebuild(&mut |_| PairView { opt_in }, &mut log);
    root.layout(Size::new(200.0, 200.0));
    (root, log)
}

fn ev(phase: PointerPhase, x: f64, y: f64) -> PointerEvent {
    PointerEvent {
        phase,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    }
}

fn mouse(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(ev(phase, x, y))
}

fn touch(slot: u32, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::PointerContact {
        pointer_id: PointerId::touch(slot),
        event: ev(phase, x, y),
    }
}

/// Positions: over A, over B, and over neither.
const A: (f64, f64) = (10.0, 10.0);
const B: (f64, f64) = (10.0, 40.0);
const NOWHERE: (f64, f64) = (150.0, 150.0);

#[test]
fn slot_zero_touch_takes_the_same_path_as_a_plain_pointer() {
    use PointerPhase::{Down, Move, Up};
    let script = [
        (Down, A),
        (Move, NOWHERE),
        (Up, NOWHERE),
        (Down, NOWHERE),
        (Move, B),
        (Down, B),
        (Up, B),
    ];
    let (mut via_pointer, mut pointer_log) = root_with(false);
    let (mut via_contact, mut contact_log) = root_with(false);
    for (phase, (x, y)) in script {
        let a = via_pointer.event(&mut pointer_log, &mouse(phase, x, y));
        let b = via_contact.event(&mut contact_log, &touch(0, phase, x, y));
        assert_eq!(a, b, "{phase:?} at ({x}, {y}): same outcome");
        assert_eq!(
            via_pointer.is_pointer_captured(),
            via_contact.is_pointer_captured()
        );
        assert_eq!(via_pointer.is_focus_active(), via_contact.is_focus_active());
        assert_eq!(via_pointer.is_hover_active(), via_contact.is_hover_active());
        assert_eq!(
            via_pointer.focus_ime_generation(),
            via_contact.focus_ime_generation()
        );
    }
    // Same widgets, same phases; only the reported id differs.
    let strip = |log: &Log| -> Vec<(char, PointerPhase)> {
        log.seen.iter().map(|(t, _, p)| (*t, *p)).collect()
    };
    assert_eq!(strip(&pointer_log), strip(&contact_log));
    assert!(
        pointer_log
            .seen
            .iter()
            .all(|(_, id, _)| *id == PointerId::MOUSE)
    );
    assert!(
        contact_log
            .seen
            .iter()
            .all(|(_, id, _)| *id == PointerId::touch(0))
    );
    // The capture the contact's Down took named it as the claimant.
    via_contact.event(&mut contact_log, &touch(0, Down, A.0, A.1));
    assert_eq!(
        via_contact.pointer_capture_claimant(),
        Some(PointerId::touch(0))
    );
}

#[test]
fn an_additional_contact_with_nothing_captured_is_dropped() {
    let (mut root, mut log) = root_with(true);
    let outcome = root.event(&mut log, &touch(1, PointerPhase::Down, A.0, A.1));
    assert_eq!(outcome, EventOutcome::default());
    assert!(log.seen.is_empty(), "no widget saw the slot-1 contact");
    assert!(!root.is_pointer_captured());
    assert!(!root.is_focus_active(), "a dropped contact claims nothing");
}

#[test]
fn an_opted_in_captor_receives_the_other_contacts_on_the_captured_path() {
    use PointerPhase::{Down, Move, Up};
    let (mut root, mut log) = root_with(true);
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));

    // The second finger lands over B, yet travels the captured path to A.
    let t1 = PointerId::touch(1);
    assert!(root.event(&mut log, &touch(1, Down, B.0, B.1)).handled);
    root.event(&mut log, &touch(1, Move, NOWHERE.0, NOWHERE.1));
    root.event(&mut log, &touch(1, Up, NOWHERE.0, NOWHERE.1));
    assert_eq!(
        log.seen[1..],
        [('A', t1, Down), ('A', t1, Move), ('A', t1, Up)],
        "A saw touch(1)'s whole contact, B saw nothing"
    );
    assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));
}

#[test]
fn a_captor_that_did_not_opt_in_never_sees_the_other_contacts() {
    use PointerPhase::{Down, Move, Up};
    let (mut root, mut log) = root_with(false);
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    for phase in [Down, Move, Up] {
        let outcome = root.event(&mut log, &touch(1, phase, B.0, B.1));
        assert_eq!(
            outcome,
            EventOutcome::default(),
            "touch(1) {phase:?} dropped"
        );
    }
    assert_eq!(log.seen, [('A', PointerId::touch(0), Down)]);
    assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));
}

#[test]
fn a_touch_release_never_ends_a_mouse_capture_and_vice_versa() {
    use PointerPhase::{Cancel, Down, Move, Up};
    // A mouse drag holds A; a finger tapping elsewhere must not break it.
    let (mut root, mut log) = root_with(false);
    root.event(&mut log, &mouse(Down, A.0, A.1));
    root.event(&mut log, &touch(0, Down, B.0, B.1));
    root.event(&mut log, &touch(0, Up, B.0, B.1));
    assert_eq!(root.pointer_capture_claimant(), Some(PointerId::MOUSE));
    root.event(&mut log, &mouse(Move, NOWHERE.0, NOWHERE.1));
    assert_eq!(
        log.seen.last(),
        Some(&('A', PointerId::MOUSE, Move)),
        "the drag still reaches its captor"
    );
    assert!(log.seen.iter().all(|(tag, _, _)| *tag == 'A'));
    root.event(&mut log, &mouse(Up, NOWHERE.0, NOWHERE.1));
    assert!(!root.is_pointer_captured());

    // The mirror image: a touch drag holds B; the mouse cannot end it.
    let (mut root, mut log) = root_with(false);
    root.event(&mut log, &touch(0, Down, B.0, B.1));
    root.event(&mut log, &mouse(Up, A.0, A.1));
    root.event(&mut log, &mouse(Cancel, A.0, A.1));
    assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));
    root.event(&mut log, &touch(0, Move, NOWHERE.0, NOWHERE.1));
    assert_eq!(log.seen.last(), Some(&('B', PointerId::touch(0), Move)));
    root.event(&mut log, &touch(0, Up, NOWHERE.0, NOWHERE.1));
    assert!(!root.is_pointer_captured());
}

#[test]
fn only_the_claimants_release_clears_the_latch() {
    use PointerPhase::{Cancel, Down, Move, Up};
    let (mut root, mut log) = root_with(true);
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    // Another contact ends twice over — delivered, yet nothing releases:
    // not the root latch, and not the container's active link either.
    root.event(&mut log, &touch(1, Down, A.0, A.1));
    root.event(&mut log, &touch(1, Up, A.0, A.1));
    root.event(&mut log, &touch(2, Down, A.0, A.1));
    root.event(&mut log, &touch(2, Cancel, A.0, A.1));
    assert_eq!(root.pointer_capture_claimant(), Some(PointerId::touch(0)));
    // Still on the captured path: a claimant move far outside A reaches A.
    root.event(&mut log, &touch(0, Move, NOWHERE.0, NOWHERE.1));
    assert_eq!(log.seen.last(), Some(&('A', PointerId::touch(0), Move)));

    // The claimant's own release ends it; later contacts fall to rule (b).
    root.event(&mut log, &touch(0, Up, NOWHERE.0, NOWHERE.1));
    assert!(!root.is_pointer_captured());
    let before = log.seen.len();
    root.event(&mut log, &touch(1, Move, A.0, A.1));
    assert_eq!(
        log.seen.len(),
        before,
        "touch(1) dropped once the gesture ended"
    );
    // And the container's link went with it: a fresh contact is hit-tested.
    root.event(&mut log, &touch(0, Down, B.0, B.1));
    assert_eq!(log.seen.last(), Some(&('B', PointerId::touch(0), Down)));
}

#[test]
fn a_non_claimant_contact_never_blurs_or_moves_hover() {
    use PointerPhase::{Down, Up};
    let (mut root, mut log) = root_with(true);
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    assert!(root.is_focus_active());
    let generation = root.focus_ime_generation();
    root.event(&mut log, &touch(1, Down, NOWHERE.0, NOWHERE.1));
    root.event(&mut log, &touch(1, Up, NOWHERE.0, NOWHERE.1));
    assert!(
        root.is_focus_active(),
        "a second finger is not a tap outside"
    );
    assert_eq!(root.focus_ime_generation(), generation);
}

/// A container in front of [`Pair`] with pointer handling of its own that
/// another contact must never reach: it logs every pointer event it routes
/// (tag `'G'`) and treats a `Down` outside its child as a tap outside — an
/// explicit focus release. With `forwards_broadcasts` unset it drops every
/// broadcast instead of forwarding it (a container the forward-only walk
/// cannot pass).
struct Guard {
    inner: crate::widget::ChildPod,
    forwards_broadcasts: bool,
}
impl crate::widget::Widget for Guard {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.inner
            .layout_child(ctx, &BoxConstraints::tight(Size::new(100.0, 60.0)));
        self.inner.set_origin(Point::ZERO);
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.inner.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            if self.forwards_broadcasts {
                self.inner.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let id = ctx.pointer_id();
        ctx.state_mut::<Log>().seen.push(('G', id, p.phase));
        let inside = self.inner.contains(p.position);
        let result = if self.inner.is_active() || inside {
            self.inner.event_child(ctx, event)
        } else {
            EventResult::Ignored
        };
        if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel) {
            self.inner.set_active(false);
        }
        if p.phase == PointerPhase::Down && !inside {
            ctx.release_focus();
        }
        result
    }
}

struct GuardView {
    forwards_broadcasts: bool,
}
impl View<Log> for GuardView {
    type Element = Guard;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Guard {
        Guard {
            inner: crate::widget::ChildPod::new(Box::new(Pair {
                a: crate::widget::ChildPod::new(Box::new(Probe {
                    tag: 'A',
                    opt_in: true,
                })),
                b: crate::widget::ChildPod::new(Box::new(Probe {
                    tag: 'B',
                    opt_in: true,
                })),
            })),
            forwards_broadcasts: self.forwards_broadcasts,
        }
    }
    fn rebuild(&self, _p: &Self, _e: &mut Guard, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn guarded_root(forwards_broadcasts: bool) -> (RenderRoot<Log, GuardView>, Log) {
    let mut root: RenderRoot<Log, GuardView> = RenderRoot::new();
    let mut log = Log::default();
    root.rebuild(
        &mut |_| GuardView {
            forwards_broadcasts,
        },
        &mut log,
    );
    root.layout(Size::new(200.0, 200.0));
    (root, log)
}

#[test]
fn another_contact_reaches_only_the_captor_and_moves_no_hover_or_focus() {
    use PointerPhase::{Down, Move, Up};
    let (mut root, mut log) = guarded_root(true);
    let (t0, t1) = (PointerId::touch(0), PointerId::touch(1));
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    assert!(root.is_focus_active());
    assert_eq!(root.pointer_capture_claimant(), Some(t0));
    assert!(root.pointer_capture_contacts());
    let (focus_generation, hover) = (root.focus_ime_generation(), root.is_hover_active());

    // A second finger landing outside the guard's child: delivered to A down
    // the captured path, while the guard — whose own `Down` handling would
    // release focus on a tap outside — never runs on it.
    assert!(
        root.event(&mut log, &touch(1, Down, NOWHERE.0, NOWHERE.1))
            .handled
    );
    root.event(&mut log, &touch(1, Move, B.0, B.1));
    root.event(&mut log, &touch(1, Up, B.0, B.1));
    assert_eq!(
        log.seen,
        [
            ('G', t0, Down),
            ('A', t0, Down),
            ('A', t1, Down),
            ('A', t1, Move),
            ('A', t1, Up),
        ],
        "only the captor saw touch(1)"
    );
    assert!(root.is_focus_active(), "no tap outside was seen");
    assert_eq!(root.focus_ime_generation(), focus_generation);
    assert_eq!(root.is_hover_active(), hover);
    assert_eq!(root.pointer_capture_claimant(), Some(t0));
    assert!(root.pointer_capture_contacts());

    // The claimant itself still takes the ordinary path through the guard.
    root.event(&mut log, &touch(0, Move, NOWHERE.0, NOWHERE.1));
    assert_eq!(
        log.seen[5..],
        [('G', t0, Move), ('A', t0, Move)],
        "the claimant's own move runs every handler on the path"
    );
    root.event(&mut log, &touch(0, Up, NOWHERE.0, NOWHERE.1));
    assert!(!root.is_pointer_captured());
    assert!(!root.pointer_capture_contacts());
}

#[test]
fn a_walk_a_container_cannot_pass_falls_back_to_the_ordinary_delivery() {
    use PointerPhase::Down;
    let (mut root, mut log) = guarded_root(false);
    let (t0, t1) = (PointerId::touch(0), PointerId::touch(1));
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    // The guard drops the carrier, so it is handed the real event instead —
    // and the captor still receives it, down the captured path.
    assert!(root.event(&mut log, &touch(1, Down, B.0, B.1)).handled);
    assert_eq!(
        log.seen,
        [
            ('G', t0, Down),
            ('A', t0, Down),
            ('G', t1, Down),
            ('A', t1, Down),
        ]
    );
    assert_eq!(root.pointer_capture_claimant(), Some(t0));
}

/// A root container that takes a gesture over from its child once the
/// claimant moves more than 10 px: it cancels the child and releases it
/// through [`EventCtx::release_captured_child`]. Logs every pointer event it
/// sees (tag `'T'`); with `opt_in` it also opts into the gesture's other
/// contacts itself, ahead of its child, on the `Down`.
struct TakeOver {
    inner: crate::widget::ChildPod,
    opt_in: bool,
    down_at: Option<Point>,
    released_seen: bool,
}
impl crate::widget::Widget for TakeOver {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.inner.layout_child(ctx, bc);
        self.inner.set_origin(Point::ZERO);
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.inner.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.inner.event_child(ctx, event);
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let id = ctx.pointer_id();
        ctx.state_mut::<Log>().seen.push(('T', id, p.phase));
        match p.phase {
            PointerPhase::Down if self.down_at.is_none() => {
                self.down_at = Some(p.position);
                ctx.capture_pointer();
                if self.opt_in {
                    ctx.capture_contacts();
                }
                if self.inner.contains(p.position) {
                    self.inner.event_child(ctx, event);
                }
            }
            PointerPhase::Move => {
                let moved = self.down_at.map(|at| (p.position - at).hypot());
                if self.inner.is_active() && moved.is_some_and(|d| d > 10.0) {
                    let cancel = InputEvent::Pointer(PointerEvent {
                        phase: PointerPhase::Cancel,
                        ..*p
                    });
                    self.inner.event_child(ctx, &cancel);
                    ctx.release_captured_child(&mut self.inner);
                    self.released_seen = ctx.is_capture_released();
                } else if self.inner.is_active() {
                    self.inner.event_child(ctx, event);
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if self.inner.is_active() {
                    self.inner.event_child(ctx, event);
                    self.inner.set_active(false);
                }
                self.down_at = None;
            }
            PointerPhase::Down => {}
        }
        EventResult::Handled
    }
}

struct TakeOverView {
    opt_in: bool,
}
impl View<Log> for TakeOverView {
    type Element = TakeOver;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TakeOver {
        TakeOver {
            inner: crate::widget::ChildPod::new(Box::new(Probe {
                tag: 'A',
                opt_in: true,
            })),
            opt_in: self.opt_in,
            down_at: None,
            released_seen: false,
        }
    }
    fn rebuild(&self, _p: &Self, _e: &mut TakeOver, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn take_over_root(opt_in: bool) -> (RenderRoot<Log, TakeOverView>, Log) {
    let mut root: RenderRoot<Log, TakeOverView> = RenderRoot::new();
    let mut log = Log::default();
    root.rebuild(&mut |_| TakeOverView { opt_in }, &mut log);
    root.layout(Size::new(200.0, 200.0));
    (root, log)
}

fn take_over_widget(root: &mut RenderRoot<Log, TakeOverView>) -> &mut TakeOver {
    let id = root.root_id().expect("root built");
    root.tree
        .pod_mut(id)
        .expect("root pod")
        .widget_mut()
        .downcast_mut::<TakeOver>()
        .expect("root is a TakeOver")
}

#[test]
fn a_takeover_ends_the_contact_opt_in_but_keeps_the_claimant() {
    use PointerPhase::{Cancel, Down, Move, Up};
    let (mut root, mut log) = take_over_root(false);
    let t0 = PointerId::touch(0);
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    assert_eq!(root.pointer_capture_claimant(), Some(t0));
    assert!(root.pointer_capture_contacts(), "A opted in");

    // The claimant moves past the container's threshold: it cancels A and
    // releases it, and the release reaches the root.
    root.event(&mut log, &touch(0, Move, A.0, A.1 + 30.0));
    assert!(take_over_widget(&mut root).released_seen);
    assert_eq!(log.seen[3], ('A', t0, Cancel));
    assert_eq!(
        root.pointer_capture_claimant(),
        Some(t0),
        "the gesture is still the claimant's, now held by the container"
    );
    assert!(
        !root.pointer_capture_contacts(),
        "the widget that asked for the other contacts is gone"
    );

    // Another finger is dropped from here on — neither the container nor
    // the cancelled captor hears it.
    let before = log.seen.len();
    let outcome = root.event(&mut log, &touch(1, Down, A.0, A.1));
    assert_eq!(outcome, EventOutcome::default());
    assert_eq!(log.seen.len(), before);

    // The claimant keeps driving the container, and its release ends it.
    root.event(&mut log, &touch(0, Move, A.0, A.1 + 60.0));
    assert_eq!(log.seen.last(), Some(&('T', t0, Move)));
    root.event(&mut log, &touch(0, Up, A.0, A.1 + 60.0));
    assert!(!root.is_pointer_captured());
}

#[test]
fn a_takeover_by_the_widget_holding_the_opt_in_keeps_it() {
    use PointerPhase::{Down, Move};
    let (mut root, mut log) = take_over_root(true);
    let (t0, t1) = (PointerId::touch(0), PointerId::touch(1));
    root.event(&mut log, &touch(0, Down, A.0, A.1));
    root.event(&mut log, &touch(0, Move, A.0, A.1 + 30.0));
    assert!(
        !take_over_widget(&mut root).released_seen,
        "the container itself holds the opt-in, so releasing its child ends nothing"
    );
    assert!(root.pointer_capture_contacts());
    // The container is the captor: another finger reaches it directly.
    assert!(root.event(&mut log, &touch(1, Down, A.0, A.1)).handled);
    assert_eq!(log.seen.last(), Some(&('T', t1, Down)));
    assert_eq!(root.pointer_capture_claimant(), Some(t0));
}
