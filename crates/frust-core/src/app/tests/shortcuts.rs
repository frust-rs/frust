//! Shortcut dispatch through a real `RenderRoot`: per-paint declarations, tier order,
//! focus-path precedence, fall-through past an unreachable or declining declarer, and the
//! overlay tier reaching a floated pod through its owner's slot.

use super::*;
use crate::event::OverlayEvent;
use crate::overlay::{OverlayBand, OverlayPod};
use crate::shortcuts::{Chord, ShortcutDecl, ShortcutId, ShortcutScope};

/// One declarer's configuration; flipped between frames by a test.
#[derive(Clone)]
struct Slot {
    name: &'static str,
    id: ShortcutId,
    chord: &'static str,
    scope: ShortcutScope,
    declare: bool,
    answers: bool,
    /// Whether it consumes a focus-routed key while focused.
    eats_keys: bool,
    /// Whether answering its shortcut also claims focus.
    focuses: bool,
}

impl Slot {
    fn new(name: &'static str, chord: &'static str, scope: ShortcutScope) -> Self {
        Self {
            name,
            id: ShortcutId::next(),
            chord,
            scope,
            declare: true,
            answers: true,
            eats_keys: false,
            focuses: false,
        }
    }
}

#[derive(Default)]
struct Fixture {
    slots: Vec<Slot>,
    /// `(declarer, delivery)` for every shortcut delivery a declarer saw.
    seen: Vec<(&'static str, ShortcutId)>,
    /// Declarers that saw a plain `Key`.
    keys: Vec<&'static str>,
    /// Overlay events the owner received.
    owner: Vec<OverlayEventKind>,
    /// Labels `BuildCtx::shortcut_hints` returned on the latest rebuild.
    built_hints: Vec<String>,
    /// Labels `PaintCtx::shortcut_hints` returned during the latest paint.
    painted_hints: Vec<String>,
}

type Shared = Rc<RefCell<Fixture>>;

fn slot(shared: &Shared, name: &str) -> Slot {
    let fixture = shared.borrow();
    fixture
        .slots
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .clone()
}

fn edit(shared: &Shared, name: &str, f: impl FnOnce(&mut Slot)) {
    let mut fixture = shared.borrow_mut();
    f(fixture.slots.iter_mut().find(|s| s.name == name).unwrap());
}

/// A leaf that declares its slot every paint, answers its own id, and takes focus on `Down`.
struct Declarer {
    name: &'static str,
    shared: Shared,
}

impl crate::widget::Widget for Declarer {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 40.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        let slot = slot(&self.shared, self.name);
        if matches!(self.name, "global" | "a" | "one" | "two") {
            let hints = ctx.shortcut_hints();
            let labels = hints.iter().map(|h| h.label.clone());
            self.shared.borrow_mut().painted_hints = labels.collect();
        }
        if slot.declare {
            ctx.register_shortcut(ShortcutDecl {
                id: slot.id,
                chord: Chord::parse(slot.chord).unwrap(),
                label: slot.name.to_string(),
                scope: slot.scope,
            });
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let slot = slot(&self.shared, self.name);
        match event {
            InputEvent::Shortcut(id) => {
                self.shared.borrow_mut().seen.push((self.name, *id));
                if *id == slot.id && slot.answers {
                    if slot.focuses {
                        ctx.request_focus();
                    }
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Key(_) => {
                self.shared.borrow_mut().keys.push(self.name);
                if slot.eats_keys {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Hosts one floated pod holding a `Declarer`; forwards its overlay events into the pod.
struct FloatOwner {
    key: OverlayKey,
    input: OverlayInput,
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
        ctx.register_overlay(OverlayEntry {
            key: self.key,
            band: OverlayBand::Floating,
            input: self.input,
            outside_tap: OutsideTap::Ignore,
            keys: crate::overlay::OverlayKeys::Pass,
            window_rect: Rect::new(120.0, 0.0, 200.0, 40.0),
            pod: Rc::clone(&self.pod),
            insets: ctx.window_insets(),
        });
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Overlay(overlay) = event
            && overlay.key == self.key
        {
            self.shared.borrow_mut().owner.push(overlay.kind.clone());
            if let OverlayEventKind::Shortcut(id) = overlay.kind {
                self.pod
                    .borrow_mut()
                    .event_child(ctx, &InputEvent::Shortcut(id));
            }
        }
        EventResult::Ignored
    }
}

/// A column of children routing like `frust-widgets`' helpers, except that a child marked
/// unreachable still paints but never receives a `Shortcut` (a covered navigator page).
struct KeysRoot {
    children: Vec<(crate::widget::ChildPod, bool)>,
}

impl crate::widget::Widget for KeysRoot {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let mut y = 0.0;
        for (pod, _) in &mut self.children {
            let size = pod.layout_child(ctx, &BoxConstraints::loose(Size::new(100.0, 40.0)));
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
        }
        bc.constrain(Size::new(200.0, 200.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for (pod, _) in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            let shortcut = matches!(event, InputEvent::Shortcut(_));
            for (pod, reachable) in &mut self.children {
                if *reachable || !shortcut {
                    pod.event_child(ctx, event);
                }
            }
            return EventResult::Ignored;
        }
        if event.is_focus_routed() {
            for (pod, _) in &mut self.children {
                if pod.holds_live_focus() {
                    return pod.event_child(ctx, event);
                }
            }
            return EventResult::Ignored;
        }
        for (pod, _) in &mut self.children {
            if pod.contains(event.position()) {
                return pod.event_child(ctx, event);
            }
        }
        EventResult::Ignored
    }
}

/// A main-tree child: a `Declarer` (reachable or not), or the overlay owner.
#[derive(Clone, Copy)]
enum Child {
    Leaf(&'static str, bool),
    Float(&'static str, OverlayInput),
}

struct KeysView {
    layout: Vec<Child>,
    shared: Shared,
}

impl View<()> for KeysView {
    type Element = KeysRoot;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> KeysRoot {
        let leaf = |name| -> Box<dyn crate::widget::Widget> {
            Box::new(Declarer {
                name,
                shared: Rc::clone(&self.shared),
            })
        };
        let children = self.layout.iter().map(|child| match *child {
            Child::Leaf(name, reachable) => (crate::widget::ChildPod::new(leaf(name)), reachable),
            Child::Float(name, input) => {
                let owner = FloatOwner {
                    key: OverlayKey::next(),
                    input,
                    pod: Rc::new(RefCell::new(crate::widget::ChildPod::new(leaf(name)))),
                    shared: Rc::clone(&self.shared),
                };
                (crate::widget::ChildPod::new(Box::new(owner)), true)
            }
        });
        KeysRoot {
            children: children.collect(),
        }
    }
    fn rebuild(&self, _: &Self, _: &mut KeysRoot, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        let hints = ctx.shortcut_hints();
        let labels = hints.iter().map(|h| h.label.clone());
        self.shared.borrow_mut().built_hints = labels.collect();
        ChangeFlags::NONE
    }
}

struct Keys {
    root: RenderRoot<(), KeysView>,
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
            &mut |_: &mut ()| KeysView {
                layout: layout.clone(),
                shared: Rc::clone(&shared),
            },
            &mut (),
        );
        keys.frame();
        keys
    }

    /// Layout and paint: the paint is what refreshes the declaration snapshot.
    fn frame(&mut self) {
        self.root.layout(Size::new(200.0, 200.0));
        self.root
            .paint(&mut RecordingScene::default(), FrameTime::ZERO);
    }

    fn press(&mut self, chord: &str) -> EventOutcome {
        let chord = Chord::parse(chord).unwrap();
        let event = InputEvent::Key(KeyEvent {
            key: chord.key,
            modifiers: chord.mods,
            repeat: false,
        });
        self.root.event(&mut (), &event)
    }

    /// Who saw which delivery, by declarer name (the delivery's own declarer, not the viewer).
    fn deliveries(&self) -> Vec<(&'static str, &'static str)> {
        let fixture = self.shared.borrow();
        let owner_of = |id: ShortcutId| fixture.slots.iter().find(|s| s.id == id).unwrap().name;
        fixture
            .seen
            .iter()
            .map(|(viewer, id)| (*viewer, owner_of(*id)))
            .collect()
    }

    /// Declarers that received a delivery of their own id, in order.
    fn own_deliveries(&self) -> Vec<&'static str> {
        self.deliveries()
            .into_iter()
            .filter(|(viewer, owner)| viewer == owner)
            .map(|(viewer, _)| viewer)
            .collect()
    }

    fn clear(&mut self) {
        let mut fixture = self.shared.borrow_mut();
        fixture.seen.clear();
        fixture.keys.clear();
        fixture.owner.clear();
    }
}

#[test]
fn an_unhandled_key_reaches_an_app_global_declaration() {
    let mut keys = Keys::new(
        vec![Slot::new("global", "Ctrl+K", ShortcutScope::AppGlobal)],
        vec![Child::Leaf("global", true)],
    );
    let outcome = keys.press("Ctrl+K");
    assert!(outcome.handled, "the declarer answered");
    assert!(outcome.needs_redraw);
    assert_eq!(keys.own_deliveries(), vec!["global"]);

    keys.clear();
    assert!(
        !keys.press("Ctrl+J").handled,
        "an undeclared chord stays unhandled"
    );
    assert!(keys.deliveries().is_empty());
}

#[test]
fn a_function_key_chord_fires_through_the_declined_key_chain() {
    let mut keys = Keys::new(
        vec![Slot::new("f2", "F2", ShortcutScope::AppGlobal)],
        vec![Child::Leaf("f2", true)],
    );
    assert!(keys.press("F2").handled);
    assert_eq!(keys.own_deliveries(), vec!["f2"]);
    keys.clear();
    assert!(!keys.press("F3").handled);
}

#[test]
fn a_key_the_focus_path_consumes_never_reaches_the_table() {
    let mut eater = Slot::new("field", "Ctrl+Q", ShortcutScope::Local);
    eater.eats_keys = true;
    let mut keys = Keys::new(
        vec![
            eater,
            Slot::new("global", "Ctrl+K", ShortcutScope::AppGlobal),
        ],
        vec![Child::Leaf("field", true), Child::Leaf("global", true)],
    );
    keys.root
        .event(&mut (), &pointer(PointerPhase::Down, 10.0, 10.0));
    keys.frame();
    keys.clear();

    assert!(
        keys.press("Ctrl+K").handled,
        "the focused field consumed it"
    );
    assert_eq!(keys.shared.borrow().keys, vec!["field"]);
    assert!(keys.deliveries().is_empty(), "no shortcut was offered");

    // The same field declining the key lets the table see it.
    edit(&keys.shared, "field", |s| s.eats_keys = false);
    keys.clear();
    assert!(keys.press("Ctrl+K").handled);
    assert_eq!(keys.own_deliveries(), vec!["global"]);
}

#[test]
fn tiers_run_overlay_then_local_then_app_global_falling_through_on_a_decline() {
    let mut keys = Keys::new(
        vec![
            Slot::new("global", "Ctrl+K", ShortcutScope::AppGlobal),
            Slot::new("local", "Ctrl+K", ShortcutScope::Local),
            Slot::new("floated", "Ctrl+K", ShortcutScope::Local),
        ],
        // Painted global-first, so tier order (not paint order) is what decides.
        vec![
            Child::Leaf("global", true),
            Child::Leaf("local", true),
            Child::Float("floated", OverlayInput::Interactive),
        ],
    );
    assert!(keys.press("Ctrl+K").handled);
    assert_eq!(
        keys.own_deliveries(),
        vec!["floated"],
        "the overlay tier first"
    );

    edit(&keys.shared, "floated", |s| s.answers = false);
    keys.clear();
    assert!(keys.press("Ctrl+K").handled);
    assert_eq!(keys.own_deliveries(), vec!["floated", "local"]);

    edit(&keys.shared, "local", |s| s.answers = false);
    keys.clear();
    assert!(keys.press("Ctrl+K").handled);
    assert_eq!(keys.own_deliveries(), vec!["floated", "local", "global"]);

    edit(&keys.shared, "global", |s| s.answers = false);
    keys.clear();
    assert!(!keys.press("Ctrl+K").handled, "nobody answered");
}

#[test]
fn an_overlay_tier_delivery_reaches_the_floated_pod_through_its_owner() {
    let mut keys = Keys::new(
        vec![Slot::new("floated", "Mod+P", ShortcutScope::AppGlobal)],
        vec![Child::Float("floated", OverlayInput::Interactive)],
    );
    let floated = slot(&keys.shared, "floated");
    let stamps: Vec<_> = keys
        .root
        .shortcuts
        .entries()
        .iter()
        .map(|e| e.overlay)
        .collect();
    assert_eq!(stamps, vec![Some(keys.root.overlay_hits[0].key)]);

    assert!(keys.press("Mod+P").handled);
    assert_eq!(
        keys.shared.borrow().owner,
        vec![OverlayEventKind::Shortcut(floated.id)],
        "the owner heard an overlay event quoting its key"
    );
    assert_eq!(keys.own_deliveries(), vec!["floated"]);
}

#[test]
fn a_transparent_pods_declarations_are_unreachable() {
    let mut keys = Keys::new(
        vec![
            Slot::new("tip", "Ctrl+K", ShortcutScope::Local),
            Slot::new("global", "Ctrl+K", ShortcutScope::AppGlobal),
        ],
        vec![
            Child::Float("tip", OverlayInput::Transparent),
            Child::Leaf("global", true),
        ],
    );
    assert!(keys.press("Ctrl+K").handled);
    assert_eq!(keys.own_deliveries(), vec!["global"]);
    assert!(keys.shared.borrow().owner.is_empty());
}

#[test]
fn a_declarer_that_stops_declaring_stops_matching_after_the_next_paint() {
    let mut keys = Keys::new(
        vec![Slot::new("local", "Ctrl+K", ShortcutScope::Local)],
        vec![Child::Leaf("local", true)],
    );
    assert!(keys.press("Ctrl+K").handled);

    edit(&keys.shared, "local", |s| s.declare = false);
    keys.clear();
    assert!(
        keys.press("Ctrl+K").handled,
        "the last paint's snapshot stands"
    );
    keys.frame();
    keys.clear();
    assert!(!keys.press("Ctrl+K").handled, "the next paint dropped it");
    assert!(keys.deliveries().is_empty());
}

#[test]
fn an_unreachable_declarer_falls_through_to_the_next_candidate() {
    let mut keys = Keys::new(
        vec![
            Slot::new("global", "Ctrl+K", ShortcutScope::AppGlobal),
            Slot::new("covered", "Ctrl+K", ShortcutScope::Local),
        ],
        vec![Child::Leaf("global", true), Child::Leaf("covered", false)],
    );
    assert_eq!(keys.root.shortcuts.entries().len(), 2, "both painted");
    assert!(keys.press("Ctrl+K").handled);
    assert_eq!(keys.own_deliveries(), vec!["global"]);
    assert!(
        keys.deliveries()
            .iter()
            .all(|(viewer, _)| *viewer != "covered"),
        "the covered declarer never received a delivery"
    );
}

#[test]
fn a_duplicate_chord_in_one_tier_goes_to_the_last_declaration_first() {
    let mut keys = Keys::new(
        vec![
            Slot::new("first", "1", ShortcutScope::Local),
            Slot::new("second", "1", ShortcutScope::Local),
        ],
        vec![Child::Leaf("first", true), Child::Leaf("second", true)],
    );
    assert!(keys.press("1").handled);
    assert_eq!(keys.own_deliveries(), vec!["second"]);

    edit(&keys.shared, "second", |s| s.answers = false);
    keys.clear();
    assert!(keys.press("1").handled);
    assert_eq!(keys.own_deliveries(), vec!["second", "first"]);
}

#[test]
fn only_a_key_event_consults_the_table() {
    let mut keys = Keys::new(
        vec![Slot::new("global", "Ctrl+K", ShortcutScope::AppGlobal)],
        vec![Child::Leaf("global", true)],
    );
    keys.root.event(&mut (), &InputEvent::Housekeeping);
    keys.root
        .event(&mut (), &pointer(PointerPhase::Move, 150.0, 150.0));
    keys.root
        .event(&mut (), &InputEvent::EditCommand(EditCommand::SelectAll));
    assert!(keys.deliveries().is_empty());
    let stray = InputEvent::Overlay(OverlayEvent {
        key: OverlayKey::next(),
        kind: OverlayEventKind::Shortcut(slot(&keys.shared, "global").id),
    });
    keys.root.event(&mut (), &stray);
    assert!(
        keys.deliveries().is_empty(),
        "another owner's overlay event"
    );
}

#[test]
fn a_shortcut_answer_may_claim_focus() {
    let mut search = Slot::new("search", "Mod+F", ShortcutScope::AppGlobal);
    search.focuses = true;
    search.eats_keys = true;
    let mut keys = Keys::new(vec![search], vec![Child::Leaf("search", true)]);
    assert!(!keys.root.focus_active);
    assert!(keys.press("Mod+F").handled);
    assert!(
        keys.root.focus_active,
        "the declarer's focus claim was honoured"
    );
    keys.frame();
    keys.clear();
    keys.press("Ctrl+Z");
    assert_eq!(
        keys.shared.borrow().keys,
        vec!["search"],
        "keys now route to it"
    );
}

#[test]
fn widgets_see_the_last_paints_hints_in_paint_order_across_events() {
    let mut keys = Keys::new(
        vec![
            Slot::new("a", "Ctrl+K", ShortcutScope::AppGlobal),
            Slot::new("b", "Ctrl+J", ShortcutScope::Local),
        ],
        vec![Child::Leaf("a", true), Child::Leaf("b", false)],
    );
    let rebuild = |keys: &mut Keys| {
        let shared = Rc::clone(&keys.shared);
        keys.root.rebuild(
            &mut |_: &mut ()| KeysView {
                layout: vec![Child::Leaf("a", true), Child::Leaf("b", false)],
                shared: Rc::clone(&shared),
            },
            &mut (),
        );
    };
    assert!(keys.shared.borrow().painted_hints.is_empty(), "first paint");
    rebuild(&mut keys);
    assert_eq!(keys.shared.borrow().built_hints, ["a", "b"]);
    keys.frame();
    assert_eq!(keys.shared.borrow().painted_hints, ["a", "b"]);

    keys.press("Ctrl+K");
    rebuild(&mut keys);
    assert_eq!(keys.shared.borrow().built_hints, ["a", "b"], "event pass");

    edit(&keys.shared, "a", |s| s.declare = false);
    edit(&keys.shared, "b", |s| s.declare = false);
    rebuild(&mut keys);
    assert_eq!(
        keys.shared.borrow().built_hints,
        ["a", "b"],
        "not repainted"
    );
    keys.frame();
    rebuild(&mut keys);
    assert!(keys.shared.borrow().built_hints.is_empty());
}

#[test]
fn two_roots_on_one_thread_each_report_their_own_hints() {
    let layout = |name| vec![Child::Leaf(name, true)];
    let mut one = Keys::new(
        vec![Slot::new("one", "Ctrl+K", ShortcutScope::AppGlobal)],
        layout("one"),
    );
    let mut two = Keys::new(
        vec![Slot::new("two", "Ctrl+J", ShortcutScope::AppGlobal)],
        layout("two"),
    );
    one.frame();
    two.frame();
    one.frame();
    let labels = |keys: &Keys| -> Vec<String> {
        let hints = keys.root.shortcut_hints(|_| true);
        hints.into_iter().map(|h| h.label).collect()
    };
    assert_eq!(labels(&one), ["one"]);
    assert_eq!(labels(&two), ["two"]);
    two.frame();
    assert_eq!(keys_painted(&one), ["one"]);
    assert_eq!(keys_painted(&two), ["two"]);
}

fn keys_painted(keys: &Keys) -> Vec<String> {
    keys.shared.borrow().painted_hints.clone()
}
