use super::*;

// --- Cursor: the per-pass request channel ---------------------------------
//
// The cursor is hover's sibling and is deliberately not derived from it (the
// root's hover mirror is identity-free), so it gets its own fixture: two
// leaves that ask for different shapes, a container that can speak before or
// after routing (the last-writer case), and a capture fast-path (the drag
// case, where the shape must survive the pointer leaving the widget).

/// How the cursor fixture's container speaks relative to its children.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerCursor {
    /// Says nothing at all — the ordinary container.
    Silent,
    /// Asks *before* routing, so whatever the child asks for comes later.
    BeforeRouting(CursorIcon),
    /// Asks *after* routing, deliberately overriding its child.
    AfterRouting(CursorIcon),
}

/// A leaf that asks for one cursor while the pointer is over it and another
/// while it holds the capture — the two shapes a real draggable control wants.
struct CursorLeaf {
    hover_icon: Option<CursorIcon>,
    drag_icon: Option<CursorIcon>,
    captures: bool,
    captured: bool,
}

impl crate::widget::Widget for CursorLeaf {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 30.0))
    }
    // A cursor is resolved entirely in the event pass — this fixture never
    // paints, unlike the hover one (whose authoritative read is at paint time).
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Move => {
                if self.captured {
                    // The captured pass belongs to this widget wherever the
                    // pointer has gone — re-asking here is what keeps the drag
                    // shape alive outside its own bounds.
                    if let Some(icon) = self.drag_icon {
                        ctx.set_cursor(icon);
                    }
                    return EventResult::Handled;
                }
                let size = ctx.size();
                let inside = p.position.x >= 0.0
                    && p.position.y >= 0.0
                    && p.position.x < size.width
                    && p.position.y < size.height;
                if inside && let Some(icon) = self.hover_icon {
                    ctx.set_cursor(icon);
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if self.captures {
                    ctx.capture_pointer();
                    self.captured = true;
                }
                EventResult::Handled
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                self.captured = false;
                EventResult::Ignored
            }
        }
    }
}

/// Two stacked cursor leaves routed exactly like [`HoverPair`], plus the
/// container's own optional request.
struct CursorPair {
    top: crate::widget::ChildPod,
    bottom: crate::widget::ChildPod,
    own: ContainerCursor,
}

impl crate::widget::Widget for CursorPair {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.top.layout_child(ctx, bc);
        self.top.set_origin(Point::ZERO);
        self.bottom.layout_child(ctx, bc);
        self.bottom.set_origin(Point::new(0.0, 30.0));
        bc.max()
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let ContainerCursor::BeforeRouting(icon) = self.own {
            ctx.set_cursor(icon);
        }
        let result = self.route(ctx, event);
        if let ContainerCursor::AfterRouting(icon) = self.own {
            ctx.set_cursor(icon);
        }
        result
    }
}

impl CursorPair {
    /// The capture-first, then hit-test routing every real container does.
    fn route(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let releases = matches!(
            event,
            InputEvent::Pointer(p)
                if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
        );
        for pod in [&mut self.top, &mut self.bottom] {
            if pod.is_active() {
                let r = pod.event_child(ctx, event);
                if releases {
                    pod.set_active(false);
                }
                return r;
            }
        }
        let pos = event.position();
        for pod in [&mut self.top, &mut self.bottom] {
            if pod.contains(pos) {
                return pod.event_child(ctx, event);
            }
        }
        EventResult::Ignored
    }
}

#[derive(Clone, Copy)]
struct CursorPairView {
    own: ContainerCursor,
    captures: bool,
}

impl View<()> for CursorPairView {
    type Element = CursorPair;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CursorPair {
        CursorPair {
            top: crate::widget::ChildPod::new(Box::new(CursorLeaf {
                hover_icon: Some(CursorIcon::Pointer),
                drag_icon: Some(CursorIcon::Grabbing),
                captures: self.captures,
                captured: false,
            })),
            bottom: crate::widget::ChildPod::new(Box::new(CursorLeaf {
                hover_icon: Some(CursorIcon::Text),
                drag_icon: None,
                captures: false,
                captured: false,
            })),
            own: self.own,
        }
    }
    fn rebuild(&self, _p: &Self, _e: &mut CursorPair, _c: &mut BuildCtx<'_>) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// A `RenderRoot` over the cursor fixture.
struct CursorHarness {
    root: RenderRoot<(), CursorPairView>,
    state: (),
}

impl CursorHarness {
    fn new(own: ContainerCursor, captures: bool) -> Self {
        let mut root: RenderRoot<(), CursorPairView> = RenderRoot::new();
        let mut state = ();
        root.rebuild(
            &mut move |_: &mut ()| CursorPairView { own, captures },
            &mut state,
        );
        root.layout(Size::new(100.0, 60.0));
        CursorHarness { root, state }
    }

    fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) -> EventOutcome {
        let event = InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        });
        self.root.event(&mut self.state, &event)
    }

    fn move_over(&mut self, leaf: Leaf) {
        match leaf {
            Leaf::Top => self.dispatch(PointerPhase::Move, 50.0, 15.0),
            Leaf::Bottom => self.dispatch(PointerPhase::Move, 50.0, 45.0),
        };
    }

    fn cursor(&self) -> CursorIcon {
        self.root.cursor()
    }
}

#[test]
fn a_move_resolves_the_requested_cursor_and_absence_resolves_default() {
    let mut h = CursorHarness::new(ContainerCursor::Silent, false);
    assert_eq!(
        h.cursor(),
        CursorIcon::Default,
        "nothing has asked for anything yet"
    );

    h.move_over(Leaf::Top);
    assert_eq!(
        h.cursor(),
        CursorIcon::Pointer,
        "the request reached the root"
    );

    // Moving onto the sibling re-resolves to *its* shape with nothing cleared:
    // the pass simply has a different last writer.
    h.move_over(Leaf::Bottom);
    assert_eq!(h.cursor(), CursorIcon::Text);

    // Moving off both: the next pass has no writer at all, and absence is the
    // default rather than a stale value — the whole point of a stateless
    // request.
    h.dispatch(PointerPhase::Move, 50.0, 200.0);
    assert_eq!(
        h.cursor(),
        CursorIcon::Default,
        "a widget that stops asking falls back with nothing to clear"
    );
}

#[test]
fn a_cursor_change_does_not_ask_for_a_repaint() {
    // Applying a cursor is a platform call the shell makes after the pass, with
    // no frame involved; folding it into `needs_redraw` would repaint the whole
    // tree on every hover move.
    let mut h = CursorHarness::new(ContainerCursor::Silent, false);
    let outcome = h.dispatch(PointerPhase::Move, 50.0, 15.0);
    assert_eq!(h.cursor(), CursorIcon::Pointer);
    assert!(
        !outcome.needs_redraw,
        "a cursor request alone never schedules a frame"
    );
}

#[test]
fn the_last_writer_on_the_routed_path_wins() {
    // A container that asks before routing loses to its child: the child's
    // handler runs later in the same pass, which is what makes a specific
    // control override the generic surface behind it.
    let mut h = CursorHarness::new(ContainerCursor::BeforeRouting(CursorIcon::ColResize), false);
    h.move_over(Leaf::Top);
    assert_eq!(
        h.cursor(),
        CursorIcon::Pointer,
        "the innermost widget the route reached spoke last"
    );

    // ...and the container's own request still resolves where no child asks
    // (moving off both leaves leaves the container as the only writer).
    h.dispatch(PointerPhase::Move, 50.0, 200.0);
    assert_eq!(h.cursor(), CursorIcon::ColResize);

    // The deliberate override is the mirror case: asking *after* routing beats
    // the child.
    let mut h = CursorHarness::new(ContainerCursor::AfterRouting(CursorIcon::NotAllowed), false);
    h.move_over(Leaf::Top);
    assert_eq!(
        h.cursor(),
        CursorIcon::NotAllowed,
        "a container overriding its children asks after routing"
    );
}

#[test]
fn only_a_pointer_move_re_resolves_the_cursor() {
    let mut h = CursorHarness::new(ContainerCursor::Silent, false);
    h.move_over(Leaf::Top);
    assert_eq!(h.cursor(), CursorIcon::Pointer);

    // A press/release says nothing about the cursor, and must not blink it back
    // to `Default` for the duration of a click.
    h.dispatch(PointerPhase::Down, 50.0, 15.0);
    assert_eq!(h.cursor(), CursorIcon::Pointer, "a Down leaves it standing");
    h.dispatch(PointerPhase::Up, 50.0, 15.0);
    assert_eq!(h.cursor(), CursorIcon::Pointer, "an Up leaves it standing");

    // Neither does a scroll, a key, or the housekeeping broadcast.
    h.root.event(
        &mut h.state,
        &InputEvent::Scroll {
            position: Point::new(50.0, 15.0),
            delta: crate::event::ScrollDelta::Lines(0.0, 1.0),
        },
    );
    assert_eq!(h.cursor(), CursorIcon::Pointer);
    h.root.event(&mut h.state, &InputEvent::Housekeeping);
    assert_eq!(h.cursor(), CursorIcon::Pointer);
}

#[test]
fn a_captured_drag_keeps_the_capturing_widgets_cursor() {
    let mut h = CursorHarness::new(ContainerCursor::Silent, true);
    h.move_over(Leaf::Top);
    assert_eq!(h.cursor(), CursorIcon::Pointer);

    // Press the top leaf: it captures. The `Down` itself resolves nothing.
    h.dispatch(PointerPhase::Down, 50.0, 15.0);
    assert!(h.root.is_pointer_captured());
    assert_eq!(h.cursor(), CursorIcon::Pointer);

    // Drag inside, then well outside its own bounds and over the sibling: every
    // one of these moves routes to the captured leaf alone, so its drag shape is
    // the only request in the pass — the sibling's `Text` never gets a say.
    h.dispatch(PointerPhase::Move, 50.0, 16.0);
    assert_eq!(h.cursor(), CursorIcon::Grabbing);
    h.dispatch(PointerPhase::Move, 50.0, 45.0);
    assert_eq!(
        h.cursor(),
        CursorIcon::Grabbing,
        "a captured drag keeps its own cursor outside its bounds"
    );
    h.dispatch(PointerPhase::Move, 50.0, 500.0);
    assert_eq!(h.cursor(), CursorIcon::Grabbing);

    // Release, then a fresh uncaptured move: the ordinary hit-tested resolution
    // is back, and the drag shape is gone with nothing cleared.
    h.dispatch(PointerPhase::Up, 50.0, 45.0);
    h.move_over(Leaf::Bottom);
    assert_eq!(h.cursor(), CursorIcon::Text);
}
