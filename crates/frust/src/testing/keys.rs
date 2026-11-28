//! Keyboard helpers on [`Tester`]: press, repeat and release a chord.
//!
//! These drive chords and named keys (`"Mod+K"`, `"Shift+Tab"`, `"Escape"`). Text entry stays with
//! `Key::Character` events through [`Tester::send`], because [`Chord::parse`] lowercases a
//! character. The chord's own modifiers ride on the event; none of these helpers calls
//! [`Tester::set_modifiers`].

use frust_core::{Chord, EventOutcome, InputEvent, KeyEvent, View};

use super::Tester;

/// Both flags OR-ed, as one outcome for a press and its release.
fn fold(a: EventOutcome, b: EventOutcome) -> EventOutcome {
    EventOutcome {
        handled: a.handled || b.handled,
        needs_redraw: a.needs_redraw || b.needs_redraw,
    }
}

fn parse(chord: &str) -> Chord {
    match Chord::parse(chord) {
        Ok(chord) => chord,
        Err(err) => panic!("bad chord {chord:?}: {err}"),
    }
}

impl<S: 'static, V: View<S>> Tester<S, V> {
    fn key_event(&mut self, chord: &str, repeat: bool) -> EventOutcome {
        let chord = parse(chord);
        self.send(&InputEvent::Key(KeyEvent {
            key: chord.key,
            modifiers: chord.mods,
            repeat,
        }))
    }

    /// Press `chord` (Flutter's `sendKeyDownEvent`): a fresh `InputEvent::Key`, no pump.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse.
    #[track_caller]
    pub fn key_down(&mut self, chord: &str) -> EventOutcome {
        self.key_event(chord, false)
    }

    /// Auto-repeat `chord` while held (Flutter's `sendKeyRepeatEvent`): an `InputEvent::Key` with
    /// `repeat` set, no pump.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse.
    #[track_caller]
    pub fn key_repeat(&mut self, chord: &str) -> EventOutcome {
        self.key_event(chord, true)
    }

    /// Release `chord` (Flutter's `sendKeyUpEvent`): an `InputEvent::KeyUp`, no pump.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse.
    #[track_caller]
    pub fn key_up(&mut self, chord: &str) -> EventOutcome {
        let chord = parse(chord);
        self.send(&InputEvent::KeyUp(KeyEvent {
            key: chord.key,
            modifiers: chord.mods,
            repeat: false,
        }))
    }

    /// Press then release `chord` (Flutter's `sendKeyEvent`); the outcomes are OR-folded.
    ///
    /// # Panics
    ///
    /// When `chord` does not parse.
    #[track_caller]
    pub fn key(&mut self, chord: &str) -> EventOutcome {
        let down = self.key_down(chord);
        let up = self.key_up(chord);
        fold(down, up)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use frust_core::{
        BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventOutcome, EventResult, InputEvent,
        Key, LayoutCtx, PaintCtx, PaintScene, PointerPhase, View, Widget,
    };
    use kurbo::Size;

    use crate::shortcuts;
    use crate::testing::Tester;

    const SIZE: Size = Size::new(200.0, 200.0);

    #[derive(Clone, Debug, PartialEq)]
    enum Seen {
        Down(Key, bool),
        Up(Key),
    }

    /// Takes focus on a pointer Down and records the keys it sees without consuming them.
    struct Probe(Rc<RefCell<Vec<Seen>>>);

    impl<S: 'static> View<S> for Probe {
        type Element = Probe;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Probe {
            Probe(Rc::clone(&self.0))
        }

        fn rebuild(&self, _prev: &Self, _el: &mut Probe, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for Probe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(SIZE)
        }

        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    ctx.request_focus();
                    EventResult::Handled
                }
                InputEvent::Key(k) => {
                    self.0
                        .borrow_mut()
                        .push(Seen::Down(k.key.clone(), k.repeat));
                    EventResult::Ignored
                }
                InputEvent::KeyUp(k) => {
                    self.0.borrow_mut().push(Seen::Up(k.key.clone()));
                    EventResult::Ignored
                }
                _ => EventResult::Ignored,
            }
        }
    }

    fn focused_probe() -> (Tester<(), impl View<()>>, Rc<RefCell<Vec<Seen>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let probe = Rc::clone(&seen);
        let mut tester = Tester::new((), move |_: &mut ()| Probe(Rc::clone(&probe)), SIZE);
        tester.tap((50.0, 50.0));
        (tester, seen)
    }

    #[derive(Default)]
    struct Count(u32);

    fn counted_view(shared: &Rc<RefCell<Vec<Seen>>>) -> impl View<Count> + use<> {
        shortcuts(Probe(Rc::clone(shared))).on("Ctrl+K", "Count", |s: &mut Count| s.0 += 1)
    }

    #[test]
    fn key_down_then_key_up_reaches_a_focused_probe_as_key_then_key_up() {
        let (mut tester, seen) = focused_probe();
        tester.key_down("Escape");
        tester.key_up("Escape");
        assert_eq!(
            *seen.borrow(),
            [
                Seen::Down(Key::Named(frust_core::NamedKey::Escape), false),
                Seen::Up(Key::Named(frust_core::NamedKey::Escape)),
            ]
        );
    }

    #[test]
    fn key_fires_a_shortcut_once_and_leaves_nothing_held() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut tester = Tester::new(
            Count::default(),
            move |_: &mut Count| counted_view(&seen),
            SIZE,
        );
        tester.tap((50.0, 50.0));
        tester.key("Ctrl+K");
        assert_eq!(tester.state().0, 1);
        assert_eq!(tester.cancel_held_keys(), EventOutcome::default());
    }

    #[test]
    fn key_repeat_refires_a_press_binding() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut tester = {
            let shared = Rc::clone(&seen);
            Tester::new(
                Count::default(),
                move |_: &mut Count| counted_view(&shared),
                SIZE,
            )
        };
        tester.tap((50.0, 50.0));
        tester.key_down("Ctrl+K");
        tester.key_repeat("Ctrl+K");
        assert_eq!(tester.state().0, 2);
        assert!(matches!(seen.borrow().last(), Some(Seen::Down(_, true))));
        tester.key_up("Ctrl+K");
    }

    #[test]
    fn cancel_held_keys_delivers_the_pending_key_up() {
        let (mut tester, seen) = focused_probe();
        tester.key_down("A");
        assert_eq!(seen.borrow().len(), 1);
        tester.cancel_held_keys();
        assert_eq!(
            seen.borrow().last(),
            Some(&Seen::Up(Key::Character("a".to_string())))
        );
    }

    #[test]
    #[should_panic(expected = "bad chord")]
    fn key_down_panics_on_a_bad_chord() {
        let (mut tester, _) = focused_probe();
        tester.key_down("Ctrl+");
    }
}
