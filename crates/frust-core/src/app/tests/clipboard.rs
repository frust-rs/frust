//! Clipboard write and paste-request channel tests for RenderRoot.

use super::*;

// Fixture: two stacked leaves under a container that can speak before or after
// routing (last-writer case) or ask for a paste of its own (two channels, one pass).

/// How the clipboard fixture's container speaks relative to its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerClipboard {
    /// Says nothing at all — the ordinary container.
    Silent,
    /// Writes *before* routing, so whatever the child writes comes later.
    WritesBeforeRouting(&'static str),
    /// Writes *after* routing, deliberately overriding its child.
    WritesAfterRouting(&'static str),
    /// Asks for a paste before routing — a toolbar refreshing whether its
    /// paste button should be enabled, which is how a write and a request
    /// legitimately ride one pass.
    AsksForPaste,
}

/// A focusable editable stand-in: claims focus on a `Down`, answers a
/// copy/cut by writing its "selection", and asks for the clipboard when it
/// sees the hardware [`NamedKey::Paste`] key it decoded itself.
struct ClipboardLeaf {
    /// What this leaf would copy.
    text: &'static str,
    /// Every [`EditCommand`] this leaf was handed, in order — the proof of
    /// who the focus routing actually reached.
    seen: Rc<RefCell<Vec<EditCommand>>>,
}

impl crate::widget::Widget for ClipboardLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 30.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                EventResult::Handled
            }
            InputEvent::EditCommand(cmd) => {
                self.seen.borrow_mut().push(cmd.clone());
                match cmd {
                    // The widget owns the selection, so it is the only thing
                    // that can say what "copy" means.
                    EditCommand::Copy | EditCommand::Cut => {
                        ctx.write_clipboard(self.text.to_string());
                    }
                    // A paste arrives with its text already read by the shell,
                    // and select-all touches no clipboard at all.
                    EditCommand::Paste(_) | EditCommand::SelectAll => {}
                }
                EventResult::Handled
            }
            // The hardware clipboard key, decoded by the widget rather than by
            // the shell: it carries no text, so the widget asks for some.
            InputEvent::Key(k) if k.key == Key::Named(NamedKey::Paste) => {
                ctx.request_paste();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

/// Two stacked clipboard leaves, routed by focus for a focus-routed event and
/// by hit test for a pointer one, plus the container's own optional request.
struct ClipboardPair {
    top: crate::widget::ChildPod,
    bottom: crate::widget::ChildPod,
    own: ContainerClipboard,
}

impl crate::widget::Widget for ClipboardPair {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.top.layout_child(ctx, bc);
        self.top.set_origin(Point::ZERO);
        self.bottom.layout_child(ctx, bc);
        self.bottom.set_origin(Point::new(0.0, 30.0));
        bc.max()
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match self.own {
            ContainerClipboard::WritesBeforeRouting(text) => ctx.write_clipboard(text.to_string()),
            ContainerClipboard::AsksForPaste => ctx.request_paste(),
            _ => {}
        }
        let result = self.route(ctx, event);
        if let ContainerClipboard::WritesAfterRouting(text) = self.own {
            ctx.write_clipboard(text.to_string());
        }
        result
    }
}

impl ClipboardPair {
    /// Focus routing for a focus-routed event, hit testing for the rest —
    /// the two branches every real container has.
    fn route(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_focus_routed() {
            for pod in [&mut self.top, &mut self.bottom] {
                if pod.is_focused() {
                    return pod.event_child(ctx, event);
                }
            }
            return EventResult::Ignored;
        }
        let pos = event.position();
        let blurs = matches!(
            event,
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down
        );
        let mut result = EventResult::Ignored;
        let mut hit = false;
        for pod in [&mut self.top, &mut self.bottom] {
            if !hit && pod.contains(pos) {
                hit = true;
                result = pod.event_child(ctx, event);
            } else if blurs {
                // A `Down` that lands elsewhere blurs the chain, exactly as
                // `frust-widgets`' routers do.
                pod.set_focused(false);
            }
        }
        result
    }
}

/// The fixture's view. It carries the two `seen` logs rather than letting the
/// harness reach into the built tree for them: `Widget` has no downcast, and
/// a shared handle is the same way the hover fixture's probe is watched.
#[derive(Clone)]
struct ClipboardPairView {
    own: ContainerClipboard,
    top_seen: Rc<RefCell<Vec<EditCommand>>>,
    bottom_seen: Rc<RefCell<Vec<EditCommand>>>,
}

impl View<()> for ClipboardPairView {
    type Element = ClipboardPair;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ClipboardPair {
        ClipboardPair {
            top: crate::widget::ChildPod::new(Box::new(ClipboardLeaf {
                text: "top selection",
                seen: self.top_seen.clone(),
            })),
            bottom: crate::widget::ChildPod::new(Box::new(ClipboardLeaf {
                text: "bottom selection",
                seen: self.bottom_seen.clone(),
            })),
            own: self.own,
        }
    }
    fn rebuild(&self, _p: &Self, _e: &mut ClipboardPair, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// A `RenderRoot` over the clipboard fixture, plus the two leaves' `seen`
/// logs (cloned out of the built widgets, which the root owns).
struct ClipboardHarness {
    root: RenderRoot<(), ClipboardPairView>,
    state: (),
    top_seen: Rc<RefCell<Vec<EditCommand>>>,
    bottom_seen: Rc<RefCell<Vec<EditCommand>>>,
}

impl ClipboardHarness {
    fn new(own: ContainerClipboard) -> Self {
        let mut root: RenderRoot<(), ClipboardPairView> = RenderRoot::new();
        let mut state = ();
        let view = ClipboardPairView {
            own,
            top_seen: Rc::new(RefCell::new(Vec::new())),
            bottom_seen: Rc::new(RefCell::new(Vec::new())),
        };
        let (top_seen, bottom_seen) = (view.top_seen.clone(), view.bottom_seen.clone());
        root.rebuild(&mut move |_: &mut ()| view.clone(), &mut state);
        root.layout(Size::new(100.0, 60.0));
        ClipboardHarness {
            root,
            state,
            top_seen,
            bottom_seen,
        }
    }

    /// Focus a leaf the way a user does: a `Down` inside its bounds.
    fn focus(&mut self, leaf: Leaf) {
        let y = match leaf {
            Leaf::Top => 15.0,
            Leaf::Bottom => 45.0,
        };
        self.root
            .event(&mut self.state, &pointer(PointerPhase::Down, 50.0, y));
    }

    fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
        self.root.event(&mut self.state, event)
    }
}

#[test]
fn a_copy_reaches_the_focused_leaf_alone_and_its_text_drains_once() {
    let mut h = ClipboardHarness::new(ContainerClipboard::Silent);
    h.focus(Leaf::Top);
    assert!(
        h.root.take_clipboard_write().is_none(),
        "a focusing tap writes nothing"
    );

    h.dispatch(&InputEvent::EditCommand(EditCommand::Copy));
    assert_eq!(
        h.top_seen.borrow().as_slice(),
        &[EditCommand::Copy],
        "the focused leaf received the command"
    );
    assert!(
        h.bottom_seen.borrow().is_empty(),
        "and the unfocused sibling never saw it — focus routing, not hit testing"
    );
    assert_eq!(
        h.root.take_clipboard_write().as_deref(),
        Some("top selection")
    );
    assert_eq!(
        h.root.take_clipboard_write(),
        None,
        "the drain is one-shot: a clipboard write is an edge, not a level"
    );
}

#[test]
fn a_copy_follows_the_focus_when_it_moves() {
    let mut h = ClipboardHarness::new(ContainerClipboard::Silent);
    h.focus(Leaf::Top);
    h.focus(Leaf::Bottom);
    h.dispatch(&InputEvent::EditCommand(EditCommand::Cut));
    assert!(
        h.top_seen.borrow().is_empty(),
        "the blurred leaf is out of the routed path"
    );
    assert_eq!(h.bottom_seen.borrow().as_slice(), &[EditCommand::Cut]);
    assert_eq!(
        h.root.take_clipboard_write().as_deref(),
        Some("bottom selection")
    );
}

#[test]
fn a_paste_request_drains_once_and_its_answer_is_an_ordinary_dispatch() {
    let mut h = ClipboardHarness::new(ContainerClipboard::Silent);
    h.focus(Leaf::Top);
    assert!(
        !h.root.take_paste_request(),
        "a focusing tap asks for nothing"
    );

    // The widget decoded the hardware Paste key itself and asked the shell.
    h.dispatch(&InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Paste),
        modifiers: Modifiers::default(),
        repeat: false,
    }));
    assert!(h.root.take_paste_request());
    assert!(
        !h.root.take_paste_request(),
        "the flag is one-shot, so a shell answers a request once"
    );

    // The shell's answer is a new, focus-routed dispatch carrying the text.
    h.dispatch(&InputEvent::EditCommand(EditCommand::Paste(
        "from the host".to_string(),
    )));
    assert_eq!(
        h.top_seen.borrow().as_slice(),
        &[EditCommand::Paste("from the host".to_string())]
    );
    assert!(
        !h.root.take_paste_request(),
        "answering a request does not raise a new one"
    );
    assert!(
        h.root.take_clipboard_write().is_none(),
        "and a paste writes nothing back to the host clipboard"
    );
}

#[test]
fn a_paste_answered_after_a_blur_reaches_nobody() {
    let mut h = ClipboardHarness::new(ContainerClipboard::Silent);
    h.focus(Leaf::Top);
    h.dispatch(&InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Paste),
        modifiers: Modifiers::default(),
        repeat: false,
    }));
    assert!(h.root.take_paste_request());

    // Focus is released while the shell's clipboard read is in flight: a
    // `Down` on empty chrome past both leaves blurs the chain.
    h.dispatch(&pointer(PointerPhase::Down, 50.0, 100.0));
    assert!(!h.root.is_focus_active(), "the tap blurred the field");

    // The shell answers anyway; focus was released by the tap on empty chrome, so
    // the focus-routed paste reaches no widget.
    h.dispatch(&InputEvent::EditCommand(EditCommand::Paste(
        "from the host".to_string(),
    )));
    assert!(h.top_seen.borrow().is_empty());
    assert!(h.bottom_seen.borrow().is_empty());
}

#[test]
fn the_last_clipboard_write_of_a_pass_wins_at_the_root() {
    // A container that writes BEFORE routing yields to its child, exactly as
    // `set_cursor` does: the innermost widget the route reaches speaks last.
    let mut h = ClipboardHarness::new(ContainerClipboard::WritesBeforeRouting("container"));
    h.focus(Leaf::Top);
    h.dispatch(&InputEvent::EditCommand(EditCommand::Copy));
    assert_eq!(
        h.root.take_clipboard_write().as_deref(),
        Some("top selection")
    );

    // A container that writes AFTER routing deliberately overrides it.
    let mut h = ClipboardHarness::new(ContainerClipboard::WritesAfterRouting("container"));
    h.focus(Leaf::Top);
    h.dispatch(&InputEvent::EditCommand(EditCommand::Copy));
    assert_eq!(h.root.take_clipboard_write().as_deref(), Some("container"));
}

#[test]
fn a_write_and_a_paste_request_ride_one_pass_independently() {
    let mut h = ClipboardHarness::new(ContainerClipboard::AsksForPaste);
    h.focus(Leaf::Top);
    // The focusing tap already carried the container's ask; drain it so the
    // assertion below is about the copy pass alone.
    assert!(h.root.take_paste_request());

    h.dispatch(&InputEvent::EditCommand(EditCommand::Copy));
    assert_eq!(
        h.root.take_clipboard_write().as_deref(),
        Some("top selection"),
        "the leaf's write landed"
    );
    assert!(
        h.root.take_paste_request(),
        "and the container's request landed in the same pass"
    );
}

#[test]
fn a_pass_that_asks_for_neither_leaves_both_empty() {
    let mut h = ClipboardHarness::new(ContainerClipboard::Silent);
    h.focus(Leaf::Top);
    // A select-all is a real clipboard verb that touches no clipboard, and a
    // pointer move touches nothing at all.
    h.dispatch(&InputEvent::EditCommand(EditCommand::SelectAll));
    h.dispatch(&pointer(PointerPhase::Move, 50.0, 15.0));
    assert_eq!(h.top_seen.borrow().as_slice(), &[EditCommand::SelectAll]);
    assert_eq!(h.root.take_clipboard_write(), None);
    assert!(!h.root.take_paste_request());
}

#[test]
fn an_edit_command_moves_focus_only_when_the_dispatch_asks() {
    // The bookkeeping arm `EditCommand` shares with `Key`/`Ime`: unlike a
    // `Down`, it neither claims nor blurs by itself.
    let mut h = ClipboardHarness::new(ContainerClipboard::Silent);
    h.focus(Leaf::Top);
    assert!(h.root.is_focus_active());
    let gen_before = h.root.focus_ime_generation();
    h.dispatch(&InputEvent::EditCommand(EditCommand::Copy));
    assert!(
        h.root.is_focus_active(),
        "a clipboard verb leaves the focus session exactly as it found it"
    );
    assert_eq!(h.root.focus_ime_generation(), gen_before);
}
