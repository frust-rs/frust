//! Key release through a real `RenderRoot`: the held set, `KeyUp` down the focus path once per
//! press, the release of the shortcut that answered the press (bare or through a floated pod's
//! owner), self-healing re-presses, the cap, and `cancel_held_keys`.

use super::*;
use crate::overlay::{OverlayBand, OverlayKeys, OverlayPod};
use crate::shortcuts::{Chord, ShortcutDecl, ShortcutId, ShortcutScope};

/// One leaf's configuration; flipped between events by a test.
#[derive(Clone)]
struct Slot {
    name: &'static str,
    id: ShortcutId,
    chord: Option<&'static str>,
    scope: ShortcutScope,
    answers: bool,
    /// Whether it consumes a focus-routed `Key` press.
    eats_keys: bool,
    /// Whether it consumes a `KeyUp`.
    eats_ups: bool,
}

impl Slot {
    fn new(name: &'static str, chord: Option<&'static str>, scope: ShortcutScope) -> Self {
        Self {
            name,
            id: ShortcutId::next(),
            chord,
            scope,
            answers: true,
            eats_keys: false,
            eats_ups: false,
        }
    }
}

/// What one leaf saw.
#[derive(Clone, Debug, PartialEq)]
enum Seen {
    Key(Key, bool),
    Up(Key, Modifiers),
    Shortcut(ShortcutId),
    Released(ShortcutId),
}

#[derive(Default)]
struct Fixture {
    slots: Vec<Slot>,
    /// Every event a leaf saw, in arrival order.
    log: Vec<(&'static str, Seen)>,
    /// Overlay events the owner received.
    owner: Vec<OverlayEventKind>,
    /// Whether the floated surface registers as modal.
    modal: bool,
}

type Shared = Rc<RefCell<Fixture>>;

fn slot(shared: &Shared, name: &str) -> Slot {
    let fixture = shared.borrow();
    let found = fixture.slots.iter().find(|s| s.name == name);
    found.unwrap().clone()
}

fn edit(shared: &Shared, name: &str, f: impl FnOnce(&mut Slot)) {
    let mut fixture = shared.borrow_mut();
    f(fixture.slots.iter_mut().find(|s| s.name == name).unwrap());
}

/// A leaf that declares its chord every paint, records what it sees, and takes focus on `Down`.
struct Leaf {
    name: &'static str,
    shared: Shared,
}

impl crate::widget::Widget for Leaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 40.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let slot = slot(&self.shared, self.name);
        if let Some(chord) = slot.chord {
            ctx.register_shortcut(ShortcutDecl {
                id: slot.id,
                chord: Chord::parse(chord).unwrap(),
                label: slot.name.to_string(),
                scope: slot.scope,
            });
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let slot = slot(&self.shared, self.name);
        let (seen, handled) = match event {
            InputEvent::Key(key) => (Seen::Key(key.key.clone(), key.repeat), slot.eats_keys),
            InputEvent::KeyUp(key) => (Seen::Up(key.key.clone(), key.modifiers), slot.eats_ups),
            InputEvent::Shortcut(id) => (Seen::Shortcut(*id), *id == slot.id && slot.answers),
            InputEvent::ShortcutReleased(id) => (Seen::Released(*id), false),
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                return EventResult::Handled;
            }
            _ => return EventResult::Ignored,
        };
        self.shared.borrow_mut().log.push((self.name, seen));
        if handled {
            EventResult::Handled
        } else {
            EventResult::Ignored
        }
    }
}

/// Hosts one floated pod holding a `Leaf`; forwards its shortcut events into the pod.
struct FloatOwner {
    key: OverlayKey,
    pod: OverlayPod,
    shared: Shared,
}

impl crate::widget::Widget for FloatOwner {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let window = BoxConstraints::loose(ctx.window_size());
        self.pod.borrow_mut().layout_child(ctx, &window);
        bc.constrain(Size::new(100.0, 10.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let modal = self.shared.borrow().modal;
        ctx.register_overlay(OverlayEntry {
            key: self.key,
            band: OverlayBand::Floating,
            input: OverlayInput::Interactive,
            outside_tap: OutsideTap::Ignore,
            keys: if modal {
                OverlayKeys::Modal
            } else {
                OverlayKeys::Pass
            },
            window_rect: Rect::new(120.0, 0.0, 200.0, 40.0),
            pod: Rc::clone(&self.pod),
            insets: ctx.window_insets(),
        });
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Overlay(overlay) = event else {
            return EventResult::Ignored;
        };
        if overlay.key != self.key {
            return EventResult::Ignored;
        }
        self.shared.borrow_mut().owner.push(overlay.kind.clone());
        let local = match overlay.kind {
            OverlayEventKind::Shortcut(id) => InputEvent::Shortcut(id),
            OverlayEventKind::ShortcutReleased(id) => InputEvent::ShortcutReleased(id),
            _ => return EventResult::Ignored,
        };
        self.pod.borrow_mut().event_child(ctx, &local);
        EventResult::Ignored
    }
}

/// A column of children routing like `frust-widgets`' helpers.
struct Column {
    children: Vec<crate::widget::ChildPod>,
}

impl crate::widget::Widget for Column {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let mut y = 0.0;
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &BoxConstraints::loose(Size::new(100.0, 40.0)));
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
        }
        bc.constrain(Size::new(200.0, 200.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.children {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if event.is_focus_routed() {
            for pod in &mut self.children {
                if pod.holds_live_focus() {
                    return pod.event_child(ctx, event);
                }
            }
            return EventResult::Ignored;
        }
        for pod in &mut self.children {
            if pod.contains(event.position()) {
                return pod.event_child(ctx, event);
            }
        }
        EventResult::Ignored
    }
}

/// A main-tree child: a `Leaf`, or a floated `Leaf` behind its owner.
#[derive(Clone, Copy)]
enum Child {
    Leaf(&'static str),
    Float(&'static str),
}

struct ColumnView {
    layout: Vec<Child>,
    shared: Shared,
}

impl View<()> for ColumnView {
    type Element = Column;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Column {
        let leaf = |name| -> Box<dyn crate::widget::Widget> {
            Box::new(Leaf {
                name,
                shared: Rc::clone(&self.shared),
            })
        };
        let children = self.layout.iter().map(|child| match *child {
            Child::Leaf(name) => crate::widget::ChildPod::new(leaf(name)),
            Child::Float(name) => crate::widget::ChildPod::new(Box::new(FloatOwner {
                key: OverlayKey::next(),
                pod: Rc::new(RefCell::new(crate::widget::ChildPod::new(leaf(name)))),
                shared: Rc::clone(&self.shared),
            })),
        });
        Column {
            children: children.collect(),
        }
    }
    fn rebuild(&self, _: &Self, _: &mut Column, _: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

struct Keys {
    root: RenderRoot<(), ColumnView>,
    shared: Shared,
}

impl Keys {
    fn new(slots: Vec<Slot>, layout: Vec<Child>) -> Self {
        let shared: Shared = Rc::new(RefCell::new(Fixture {
            slots,
            ..Fixture::default()
        }));
        let mut keys = Self {
            root: RenderRoot::new(),
            shared: Rc::clone(&shared),
        };
        keys.root.rebuild(
            &mut |_: &mut ()| ColumnView {
                layout: layout.clone(),
                shared: Rc::clone(&shared),
            },
            &mut (),
        );
        keys.frame();
        keys
    }

    fn frame(&mut self) {
        self.root.layout(Size::new(200.0, 200.0));
        self.root
            .paint(&mut RecordingScene::default(), FrameTime::ZERO);
    }

    /// Focus the main-tree child at row `row` with a tap, then repaint.
    fn focus_row(&mut self, row: usize) {
        let y = row as f64 * 40.0 + 10.0;
        self.root
            .event(&mut (), &pointer(PointerPhase::Down, 10.0, y));
        self.root
            .event(&mut (), &pointer(PointerPhase::Up, 10.0, y));
        self.frame();
        self.clear();
    }

    fn key_event(chord: &str, repeat: bool) -> KeyEvent {
        let chord = Chord::parse(chord).unwrap();
        KeyEvent {
            key: chord.key,
            modifiers: chord.mods,
            repeat,
        }
    }

    fn press(&mut self, chord: &str) -> EventOutcome {
        let event = InputEvent::Key(Self::key_event(chord, false));
        self.root.event(&mut (), &event)
    }

    fn repeat(&mut self, chord: &str) -> EventOutcome {
        let event = InputEvent::Key(Self::key_event(chord, true));
        self.root.event(&mut (), &event)
    }

    fn release(&mut self, chord: &str) -> EventOutcome {
        let event = InputEvent::KeyUp(Self::key_event(chord, false));
        self.root.event(&mut (), &event)
    }

    fn press_key(&mut self, key: Key) -> EventOutcome {
        let event = InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::NONE,
            repeat: false,
        });
        self.root.event(&mut (), &event)
    }

    fn release_key(&mut self, key: Key) -> EventOutcome {
        let event = InputEvent::KeyUp(KeyEvent {
            key,
            modifiers: Modifiers::NONE,
            repeat: false,
        });
        self.root.event(&mut (), &event)
    }

    fn id(&self, name: &str) -> ShortcutId {
        slot(&self.shared, name).id
    }

    fn log(&self) -> Vec<(&'static str, Seen)> {
        self.shared.borrow().log.clone()
    }

    /// What leaf `name` saw, in order.
    fn seen_by(&self, name: &str) -> Vec<Seen> {
        let log = self.log();
        let mine = log.into_iter().filter(|(viewer, _)| *viewer == name);
        mine.map(|(_, seen)| seen).collect()
    }

    /// The shortcut releases leaf `name` saw of its own id.
    fn own_releases(&self, name: &str) -> usize {
        let id = self.id(name);
        let seen = self.seen_by(name);
        seen.iter().filter(|s| **s == Seen::Released(id)).count()
    }

    fn clear(&mut self) {
        let mut fixture = self.shared.borrow_mut();
        fixture.log.clear();
        fixture.owner.clear();
    }
}

fn character(c: &str) -> Key {
    Key::Character(c.to_string())
}

fn global(name: &'static str, chord: &'static str) -> Slot {
    Slot::new(name, Some(chord), ShortcutScope::AppGlobal)
}

fn field(name: &'static str) -> Slot {
    Slot::new(name, None, ShortcutScope::Local)
}

#[test]
fn a_key_up_reaches_the_focus_path_once_per_press() {
    let mut eater = field("field");
    eater.eats_ups = true;
    let mut keys = Keys::new(vec![eater], vec![Child::Leaf("field")]);
    keys.focus_row(0);

    keys.press("A");
    let up = keys.release("A");
    assert!(up.handled, "the focused field consumed the release");
    assert!(up.needs_redraw);
    assert_eq!(
        keys.seen_by("field"),
        vec![
            Seen::Key(character("a"), false),
            Seen::Up(character("a"), Modifiers::NONE)
        ]
    );

    keys.clear();
    assert_eq!(
        keys.release("A"),
        EventOutcome::default(),
        "already released"
    );
    assert!(keys.log().is_empty());
}

#[test]
fn a_key_up_for_an_unrecorded_key_is_dropped() {
    let mut eater = field("field");
    eater.eats_ups = true;
    let mut keys = Keys::new(vec![eater], vec![Child::Leaf("field")]);
    keys.focus_row(0);
    assert_eq!(keys.release("Ctrl+K"), EventOutcome::default());
    assert!(keys.log().is_empty(), "nothing was dispatched");
    assert!(keys.root.held_keys().is_empty());
}

#[test]
fn a_key_up_matches_its_press_case_insensitively() {
    let mut keys = Keys::new(vec![field("field")], vec![Child::Leaf("field")]);
    keys.focus_row(0);
    keys.press_key(character("K"));
    keys.release_key(character("k"));
    assert_eq!(
        keys.seen_by("field"),
        vec![
            Seen::Key(character("K"), false),
            Seen::Up(character("K"), Modifiers::NONE)
        ],
        "the release names the key as it was pressed"
    );
    assert!(keys.root.held_keys().is_empty());
}

#[test]
fn an_answered_shortcut_gets_one_release_on_key_up() {
    let mut keys = Keys::new(
        vec![global("global", "Ctrl+K"), field("other")],
        vec![Child::Leaf("global"), Child::Leaf("other")],
    );
    let id = keys.id("global");
    assert!(keys.press("Ctrl+K").handled);
    keys.clear();

    let up = keys.release("Ctrl+K");
    assert!(!up.handled, "nothing on the focus path took the release");
    assert_eq!(keys.seen_by("global"), vec![Seen::Released(id)]);
    assert_eq!(
        keys.seen_by("other"),
        vec![Seen::Released(id)],
        "a broadcast reaches every mounted widget"
    );

    keys.clear();
    keys.release("Ctrl+K");
    assert!(keys.log().is_empty(), "one release per press");
}

#[test]
fn an_overlay_stamped_shortcut_is_released_through_its_owner() {
    let mut keys = Keys::new(
        vec![global("floated", "Mod+P"), field("main")],
        vec![Child::Float("floated"), Child::Leaf("main")],
    );
    let id = keys.id("floated");
    assert!(keys.press("Mod+P").handled);
    keys.clear();

    keys.release("Mod+P");
    assert_eq!(
        keys.shared.borrow().owner,
        vec![OverlayEventKind::ShortcutReleased(id)],
        "the owner heard the release quoting its key"
    );
    assert_eq!(keys.seen_by("floated"), vec![Seen::Released(id)]);
    assert!(
        keys.seen_by("main").is_empty(),
        "no bare broadcast reached the main tree"
    );
}

#[test]
fn a_modal_opened_mid_hold_does_not_swallow_the_release() {
    let mut keys = Keys::new(
        vec![global("global", "Ctrl+K"), field("modal")],
        vec![Child::Leaf("global"), Child::Float("modal")],
    );
    let id = keys.id("global");
    assert!(keys.press("Ctrl+K").handled);

    keys.shared.borrow_mut().modal = true;
    keys.frame();
    keys.clear();
    keys.release("Ctrl+K");
    assert_eq!(
        keys.seen_by("global"),
        vec![Seen::Released(id)],
        "the release goes by recorded id, past the modal's mask"
    );

    keys.clear();
    assert!(
        !keys.press("Ctrl+K").handled,
        "a fresh press is masked by the modal"
    );
    assert!(keys.seen_by("global").is_empty());
}

#[test]
fn a_repeat_redelivers_the_shortcut_and_keeps_one_held_entry() {
    let mut keys = Keys::new(
        vec![global("global", "Ctrl+K")],
        vec![Child::Leaf("global")],
    );
    let id = keys.id("global");
    keys.press("Ctrl+K");
    assert!(keys.repeat("Ctrl+K").handled);
    assert!(keys.repeat("Ctrl+K").handled);
    assert_eq!(
        keys.seen_by("global"),
        vec![Seen::Shortcut(id), Seen::Shortcut(id), Seen::Shortcut(id)],
        "a repeat re-fires and releases nothing"
    );
    assert_eq!(keys.root.held_keys(), vec![character("k")]);

    keys.clear();
    keys.release("Ctrl+K");
    assert_eq!(keys.own_releases("global"), 1);
}

#[test]
fn a_fresh_press_of_a_held_key_releases_it_first() {
    let mut keys = Keys::new(
        vec![field("field"), global("global", "Ctrl+K")],
        vec![Child::Leaf("field"), Child::Leaf("global")],
    );
    keys.focus_row(0);
    let id = keys.id("global");
    keys.press("Ctrl+K");
    keys.clear();

    assert!(keys.press("Ctrl+K").handled);
    let key = character("k");
    assert_eq!(
        keys.log(),
        vec![
            ("field", Seen::Up(key.clone(), Modifiers::NONE)),
            ("field", Seen::Released(id)),
            ("global", Seen::Released(id)),
            ("field", Seen::Key(key.clone(), false)),
            ("field", Seen::Shortcut(id)),
            ("global", Seen::Shortcut(id)),
        ],
        "the missed release is synthesized before the new press"
    );
    assert_eq!(keys.root.held_keys(), vec![key]);
}

#[test]
fn a_different_shortcut_answering_a_held_key_releases_the_old_one() {
    let mut keys = Keys::new(
        vec![
            global("global", "Ctrl+K"),
            Slot::new("local", Some("Ctrl+K"), ShortcutScope::Local),
        ],
        vec![Child::Leaf("global"), Child::Leaf("local")],
    );
    let (global_id, local_id) = (keys.id("global"), keys.id("local"));
    keys.press("Ctrl+K");
    assert_eq!(keys.seen_by("local"), vec![Seen::Shortcut(local_id)]);

    edit(&keys.shared, "local", |s| s.answers = false);
    keys.clear();
    assert!(keys.repeat("Ctrl+K").handled);
    assert_eq!(
        keys.seen_by("global"),
        vec![
            Seen::Shortcut(local_id),
            Seen::Shortcut(global_id),
            Seen::Released(local_id)
        ],
        "the old shortcut is released once the new one answers"
    );

    keys.clear();
    keys.release("Ctrl+K");
    assert_eq!(keys.seen_by("global"), vec![Seen::Released(global_id)]);
}

#[test]
fn cancel_held_keys_releases_every_key_and_shortcut_once_then_is_a_no_op() {
    let mut keys = Keys::new(
        vec![field("field"), global("global", "Ctrl+K")],
        vec![Child::Leaf("field"), Child::Leaf("global")],
    );
    keys.focus_row(0);
    let id = keys.id("global");
    keys.press("A");
    keys.press("Ctrl+K");
    keys.clear();

    let outcome = keys.root.cancel_held_keys(&mut ());
    assert!(!outcome.handled, "nothing on the focus path took a release");
    assert_eq!(
        keys.log(),
        vec![
            ("field", Seen::Up(character("a"), Modifiers::NONE)),
            ("field", Seen::Up(character("k"), Modifiers::NONE)),
            ("field", Seen::Released(id)),
            ("global", Seen::Released(id)),
        ],
        "oldest first, each key's shortcut right after its key"
    );
    assert!(keys.root.held_keys().is_empty());

    keys.clear();
    assert_eq!(keys.root.cancel_held_keys(&mut ()), EventOutcome::default());
    assert!(keys.log().is_empty());
}

#[test]
fn a_key_up_after_cancel_is_dropped() {
    let mut keys = Keys::new(
        vec![field("field"), global("global", "Ctrl+K")],
        vec![Child::Leaf("field"), Child::Leaf("global")],
    );
    keys.focus_row(0);
    keys.press("Ctrl+K");
    keys.root.cancel_held_keys(&mut ());
    keys.clear();
    assert_eq!(keys.release("Ctrl+K"), EventOutcome::default());
    assert!(keys.log().is_empty());
}

#[test]
fn the_held_set_evicts_its_oldest_entry_with_a_release_at_the_cap() {
    let mut keys = Keys::new(
        vec![field("field"), global("global", "Ctrl+K")],
        vec![Child::Leaf("field"), Child::Leaf("global")],
    );
    keys.focus_row(0);
    let id = keys.id("global");
    keys.press("Ctrl+K");
    for i in 0..63 {
        keys.press_key(character(&format!("c{i}")));
    }
    assert_eq!(keys.root.held_keys().len(), 64);
    keys.clear();

    keys.press_key(character("c63"));
    assert_eq!(
        keys.log(),
        vec![
            ("field", Seen::Up(character("k"), Modifiers::NONE)),
            ("field", Seen::Released(id)),
            ("global", Seen::Released(id)),
            ("field", Seen::Key(character("c63"), false)),
        ],
        "the oldest entry is released before the new press"
    );
    let held = keys.root.held_keys();
    assert_eq!(held.len(), 64);
    assert_eq!(held.first(), Some(&character("c0")));
    assert_eq!(held.last(), Some(&character("c63")));
}

#[test]
fn a_key_up_never_consults_the_table_or_tab_traversal() {
    let mut eater = field("field");
    eater.eats_keys = true;
    let mut keys = Keys::new(
        vec![
            eater,
            field("next"),
            global("global", "Ctrl+K"),
            global("tab", "Tab"),
        ],
        vec![
            Child::Leaf("field"),
            Child::Leaf("next"),
            Child::Leaf("global"),
            Child::Leaf("tab"),
        ],
    );
    keys.focus_row(0);
    let focus_epoch = keys.root.focus_epoch;

    assert!(keys.press("Ctrl+K").handled, "the field ate the press");
    assert!(keys.press("Tab").handled, "and this one");
    keys.clear();
    assert!(!keys.release("Ctrl+K").handled);
    assert!(!keys.release("Tab").handled);
    assert_eq!(
        keys.seen_by("field"),
        vec![
            Seen::Up(character("k"), Modifiers::CTRL),
            Seen::Up(Key::Named(NamedKey::Tab), Modifiers::NONE)
        ]
    );
    assert!(
        keys.log().iter().all(|(viewer, _)| *viewer == "field"),
        "no shortcut or release reached a declarer: {:?}",
        keys.log()
    );
    assert_eq!(keys.root.focus_epoch, focus_epoch, "focus did not move");
    assert!(!keys.root.keyboard_focus);
}

#[test]
fn held_keys_lists_presses_oldest_first() {
    let mut keys = Keys::new(vec![field("field")], vec![Child::Leaf("field")]);
    keys.press("A");
    keys.press("B");
    keys.press("C");
    assert_eq!(
        keys.root.held_keys(),
        vec![character("a"), character("b"), character("c")]
    );
    keys.release("B");
    assert_eq!(keys.root.held_keys(), vec![character("a"), character("c")]);
    keys.press("A");
    assert_eq!(
        keys.root.held_keys(),
        vec![character("c"), character("a")],
        "a fresh press re-records the key as the newest"
    );
}
