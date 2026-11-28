//! Held modifiers: a shell pushes them with `set_modifiers`; any widget reads them as
//! `EventCtx::modifiers()` during every dispatch.

use super::*;
use crate::event::{ScaleEvent, ScalePhase};
use crate::shortcuts::{Chord, ShortcutDecl, ShortcutId, ShortcutScope};

type Seen = Rc<RefCell<Vec<(&'static str, Modifiers)>>>;

/// A leaf recording `ctx.modifiers()` per event, and (when asked) a fresh context's reading.
struct Probe {
    seen: Seen,
    shortcut: Option<ShortcutId>,
}

impl crate::widget::Widget for Probe {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 100.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if let Some(id) = self.shortcut {
            ctx.register_shortcut(ShortcutDecl {
                id,
                chord: Chord::parse("Ctrl+K").unwrap(),
                label: "k".to_string(),
                scope: ShortcutScope::AppGlobal,
            });
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let tag = match event {
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => "down",
                PointerPhase::Move => "move",
                PointerPhase::Up => "up",
                PointerPhase::Cancel => "cancel",
            },
            InputEvent::PointerContact { .. } => "contact",
            InputEvent::Scroll { .. } => "scroll",
            InputEvent::Scale(_) => "scale",
            InputEvent::Key(_) => "key",
            InputEvent::Shortcut(_) => "shortcut",
            _ => return EventResult::Ignored,
        };
        self.seen.borrow_mut().push((tag, ctx.modifiers()));
        if tag == "key" {
            let mut count = 0u32;
            let fresh = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
            self.seen.borrow_mut().push(("fresh", fresh.modifiers()));
            return EventResult::Ignored;
        }
        EventResult::Handled
    }
}

#[derive(Clone)]
struct ProbeView {
    seen: Seen,
    shortcut: Option<ShortcutId>,
}

impl View<()> for ProbeView {
    type Element = Probe;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Probe {
        Probe {
            seen: Rc::clone(&self.seen),
            shortcut: self.shortcut,
        }
    }
    fn rebuild(&self, _p: &Self, _e: &mut Probe, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn harness(shortcut: Option<ShortcutId>) -> (RenderRoot<(), ProbeView>, Seen) {
    let seen: Seen = Rc::default();
    let view = ProbeView {
        seen: Rc::clone(&seen),
        shortcut,
    };
    let mut root: RenderRoot<(), ProbeView> = RenderRoot::new();
    root.rebuild(&mut move |_: &mut ()| view.clone(), &mut ());
    root.layout(Size::new(100.0, 100.0));
    root.paint(&mut RecordingScene::default(), FrameTime::ZERO);
    (root, seen)
}

fn pointer(phase: PointerPhase) -> PointerEvent {
    PointerEvent {
        phase,
        position: Point::new(50.0, 50.0),
        button: PointerButton::Primary,
    }
}

fn key(modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Character("k".into()),
        modifiers,
        repeat: false,
    })
}

#[test]
fn a_fresh_root_holds_no_modifiers_and_set_reports_a_change() {
    let (mut root, _) = harness(None);
    assert_eq!(root.modifiers(), Modifiers::NONE);
    assert!(!root.set_modifiers(Modifiers::NONE));
    assert!(root.set_modifiers(Modifiers::ALT));
    assert!(!root.set_modifiers(Modifiers::ALT));
    assert_eq!(root.modifiers(), Modifiers::ALT);
    assert!(root.clear_modifiers());
    assert!(!root.clear_modifiers());
    assert_eq!(root.modifiers(), Modifiers::NONE);
}

#[test]
fn pointer_scroll_and_scale_passes_read_the_held_modifiers() {
    let (mut root, seen) = harness(None);
    root.set_modifiers(Modifiers::ALT);
    let at = Point::new(50.0, 50.0);
    let events = [
        InputEvent::Pointer(pointer(PointerPhase::Down)),
        InputEvent::Pointer(pointer(PointerPhase::Move)),
        InputEvent::Pointer(pointer(PointerPhase::Up)),
        InputEvent::PointerContact {
            pointer_id: PointerId::touch(0),
            event: pointer(PointerPhase::Down),
        },
        InputEvent::Scroll {
            position: at,
            delta: ScrollDelta::Pixels(0.0, 10.0),
        },
        InputEvent::Scale(ScaleEvent {
            phase: ScalePhase::Update,
            scale_delta: 1.1,
            focal: at,
            velocity: 0.0,
        }),
    ];
    for event in &events {
        root.event(&mut (), event);
    }
    let seen = seen.borrow();
    let tags: Vec<_> = seen.iter().map(|(t, _)| *t).collect();
    assert_eq!(tags, ["down", "move", "up", "down", "scroll", "scale"]);
    assert!(seen.iter().all(|(_, m)| *m == Modifiers::ALT), "{seen:?}");
}

#[test]
fn a_key_pass_reads_the_key_events_chord_and_leaves_the_held_state() {
    let (mut root, seen) = harness(None);
    root.set_modifiers(Modifiers::ALT);
    root.event(&mut (), &key(Modifiers::SHIFT));
    assert_eq!(seen.borrow()[0], ("key", Modifiers::SHIFT));
    assert_eq!(root.modifiers(), Modifiers::ALT);
    root.event(&mut (), &InputEvent::Pointer(pointer(PointerPhase::Move)));
    assert_eq!(seen.borrow().last().unwrap().1, Modifiers::ALT);
}

#[test]
fn a_declined_key_delivered_onward_keeps_the_key_chord() {
    let id = ShortcutId::next();
    let (mut root, seen) = harness(Some(id));
    root.set_modifiers(Modifiers::ALT);
    let chord = Chord::parse("Ctrl+K").unwrap();
    let event = InputEvent::Key(KeyEvent {
        key: chord.key,
        modifiers: chord.mods,
        repeat: false,
    });
    assert!(root.event(&mut (), &event).handled);
    let seen = seen.borrow();
    assert!(seen.contains(&("shortcut", Modifiers::CTRL)), "{seen:?}");
}

#[test]
fn a_context_built_inside_a_pass_reads_the_pass_modifiers() {
    let (mut root, seen) = harness(None);
    root.event(&mut (), &key(Modifiers::META));
    assert!(seen.borrow().contains(&("fresh", Modifiers::META)));
    let mut count = 0u32;
    let outside = EventCtx::new(&mut count, Point::ZERO, Size::ZERO);
    assert_eq!(outside.modifiers(), Modifiers::NONE);
}

#[test]
fn set_modifiers_dirties_nothing() {
    let (mut root, seen) = harness(None);
    root.take_change_flags();
    root.set_modifiers(Modifiers::SHIFT);
    root.clear_modifiers();
    assert!(!root.has_pending_change_flags());
    assert!(seen.borrow().is_empty(), "no event is delivered");
}
