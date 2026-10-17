use super::*;
use crate::event::EditingState;

// --- Focus / IME surface fixtures: a root editable that focuses + publishes
//     an IME surface on a `Down` in its left half, and blurs (no focus) on a
//     `Down` in its right half. ---

struct ImeWidget;

impl crate::widget::Widget for ImeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 100.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            if p.phase == PointerPhase::Down && p.position.x < 50.0 {
                ctx.request_focus();
                ctx.publish_ime_state(ImeState {
                    active: true,
                    editing: EditingState {
                        text: "abc".to_string(),
                        selection_base: 3,
                        selection_extent: 3,
                        composing_base: -1,
                        composing_extent: -1,
                    },
                    caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
                    content_type: Default::default(),
                    suppress_soft_keyboard: false,
                });
                return EventResult::Handled;
            }
            if p.phase == PointerPhase::Down {
                // Right-half tap: a blur (no focus request).
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }
}

struct ImeView;

impl View<ClickState> for ImeView {
    type Element = ImeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeWidget {
        ImeWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut ImeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn ime_logic(_state: &mut ClickState) -> ImeView {
    ImeView
}

#[test]
fn focus_and_ime_state_surface_and_clear_on_blur() {
    let mut root: RenderRoot<ClickState, ImeView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut ime_logic, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // No focus / no IME surface initially.
    assert!(!root.is_focus_active());
    assert!(root.ime_state().is_none());

    // A left-half Down focuses the widget and publishes an IME surface.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    assert!(root.is_focus_active());
    let ime = root
        .ime_state()
        .expect("focused widget published an IME surface");
    assert!(ime.active);
    assert_eq!(ime.editing.text, "abc");

    // The published surface survives a rebuild (shell can query it between
    // frames).
    root.rebuild(&mut ime_logic, &mut state);
    assert!(root.ime_state().is_some());

    // A right-half Down is a blur: focus and the IME surface both clear.
    root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 10.0));
    assert!(!root.is_focus_active());
    assert!(root.ime_state().is_none());
}

// --- Session release: the focus session must die with its owner -----------
//
// Two routes end a session without any user input reaching the root:
//
//  (a) a widget publishes an INACTIVE IME surface (the navigator's post-pop
//      `cleared_ime_state`, `PatternSwitcher`'s equivalent, a `TextInput`
//      turned disabled under a live focus), and
//  (b) a reconciler tears the focused pod out of the tree (any generic
//      unmount — `frust-widgets`' `cancel_active_children`/`teardown_child`),
//      which raises `mark_focus_orphaned` because it has no `RenderRoot` to
//      reach from a `BuildCtx` pass.
//
// Both must perform the SAME full release a blur does. Leaving either half
// standing — `focus_active` true, or `ime_state` parked at `Some(inactive)` —
// is what stranded a popped screen: `is_focus_active()` kept lying, the
// shell's IME poll kept seeing a surface, and the next real focus
// interaction started from a corrupt baseline.

/// The navigator's cleared surface, spelled out here so the fixture below
/// publishes exactly the shape `nav::navigator::cleared_ime_state` does
/// (`frust-core` cannot name it — `frust-widgets` sits above this crate).
fn cleared_surface() -> ImeState {
    ImeState {
        active: false,
        editing: EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: None,
        content_type: Default::default(),
        suppress_soft_keyboard: false,
    }
}

/// A focused editable that publishes its active surface from paint (like a
/// real field), and — once `clear` is raised — publishes the *inactive*
/// surface from paint instead: the container-after-a-pop shape.
struct PopImeWidget {
    clear: Rc<Cell<bool>>,
}

impl crate::widget::Widget for PopImeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 100.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if self.clear.get() {
            ctx.publish_ime_state(cleared_surface());
        } else if ctx.has_focus() {
            ctx.publish_ime_state(PaintImeWidget::surface("abc"));
        }
    }
    fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                ctx.publish_ime_state(PaintImeWidget::surface("abc"));
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

struct PopImeView {
    clear: Rc<Cell<bool>>,
}

impl View<ClickState> for PopImeView {
    type Element = PopImeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PopImeWidget {
        PopImeWidget {
            clear: self.clear.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut PopImeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

#[test]
fn inactive_paint_publish_releases_the_whole_session() {
    // (a) The pop shape. Before this, the paint take stored `Some(inactive)`
    // and never touched `focus_active`, so the session outlived the page.
    let clear = Rc::new(Cell::new(false));
    let mut build = {
        let clear = clear.clone();
        move |_state: &mut ClickState| PopImeView {
            clear: clear.clone(),
        }
    };
    let mut root: RenderRoot<ClickState, PopImeView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut build, &mut state);
    root.layout(Size::new(100.0, 100.0));

    let mut scene = RecordingScene::default();
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(root.is_focus_active());
    assert!(root.ime_state().is_some_and(|s| s.active));
    let focused = root.focus_ime_generation();

    // The "pop": the next paint publishes the cleared surface.
    clear.set(true);
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(
        !root.is_focus_active(),
        "an inactive publish ends the session, not just the surface"
    );
    assert_eq!(
        root.ime_state(),
        None,
        "the surface is dropped, never parked at Some(inactive)"
    );
    assert_eq!(
        root.focus_ime_generation(),
        focused.wrapping_add(1),
        "one release is exactly one edge"
    );

    // The widget keeps publishing the cleared surface every frame (a real
    // one-shot flag would not, but an idle screen must survive the worst
    // case): the paint take's `focus_active` guard makes each a no-op, so
    // the released session neither resurrects nor spins the edge.
    let released = root.focus_ime_generation();
    for _ in 0..30 {
        root.paint(&mut scene, FrameTime::ZERO);
    }
    assert!(!root.is_focus_active());
    assert!(root.ime_state().is_none());
    assert_eq!(
        root.focus_ime_generation(),
        released,
        "an inactive publish against an already-released root is inert"
    );
}

/// A view whose rebuild raises the generic-unmount orphan mark on demand —
/// standing in for `frust-widgets`' reconcilers, which clear a focused
/// `ChildPod` mid-diff and raise exactly this flag (this crate has no
/// multi-child container of its own to diff).
struct UnmountView {
    orphan: Rc<Cell<bool>>,
}

impl View<ClickState> for UnmountView {
    type Element = ImeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ImeWidget {
        ImeWidget
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut ImeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if self.orphan.get() {
            crate::event::mark_focus_orphaned();
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

#[test]
fn generic_unmount_orphan_releases_the_whole_session() {
    // (b) The child-list-diff shape: no publish, no event — the focused
    // widget simply stops existing. Nothing self-corrects this on an idle
    // screen, which is why the reconciler's mark is drained here.
    let _ = crate::event::take_focus_orphaned();
    let orphan = Rc::new(Cell::new(false));
    let mut build = {
        let orphan = orphan.clone();
        move |_state: &mut ClickState| UnmountView {
            orphan: orphan.clone(),
        }
    };
    let mut root: RenderRoot<ClickState, UnmountView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut build, &mut state);
    root.layout(Size::new(100.0, 100.0));

    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    assert!(root.is_focus_active());
    assert!(root.ime_state().is_some());
    let focused = root.focus_ime_generation();

    // The unmount rebuild.
    orphan.set(true);
    root.rebuild(&mut build, &mut state);
    assert!(
        !root.is_focus_active(),
        "the root's focus mirror does not outlive the widget it mirrors"
    );
    assert!(root.ime_state().is_none());
    assert_eq!(
        root.focus_ime_generation(),
        focused.wrapping_add(1),
        "one orphaned focus path is exactly one edge"
    );

    // The mark was drained, so an ordinary rebuild afterwards is inert...
    let released = root.focus_ime_generation();
    orphan.set(false);
    root.rebuild(&mut build, &mut state);
    assert_eq!(root.focus_ime_generation(), released);

    // ...and re-marking against an already-released root fires no edge
    // either (a stale `focused` flag torn down later must not spin it).
    orphan.set(true);
    root.rebuild(&mut build, &mut state);
    assert_eq!(root.focus_ime_generation(), released);
    assert!(!root.is_focus_active());
}

// --- The focus/IME EDGE generation ---------------------------------------
//
// `focus_ime_generation` is the shell-facing edge behind the mobile frame
// gate's `FrameInputs::focus_or_ime_changed`: a shell caches the value and
// runs a frame when it moves. Two properties make that safe, and both are
// pinned below: EVERY real transition moves it (or a focus change strands
// unpainted), and NO same-value write moves it (or a focused screen forces
// a frame every vsync — the level-input behavior this replaced, measured at
// 62–120 fps on a static focused screen).

/// A widget that focuses on `Down`, releases focus on any `Key`, and — the
/// point of the fixture — re-publishes an IME surface from its **paint**
/// pass on every frame, reading the text from a shared cell so a test can
/// make a republish genuinely change (or genuinely not).
struct PaintImeWidget {
    published: Rc<RefCell<String>>,
}

impl PaintImeWidget {
    fn surface(text: &str) -> ImeState {
        ImeState {
            active: true,
            editing: EditingState {
                text: text.to_string(),
                selection_base: 0,
                selection_extent: 0,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
            content_type: Default::default(),
            suppress_soft_keyboard: false,
        }
    }
}

impl crate::widget::Widget for PaintImeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 100.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.publish_ime_state(Self::surface(&self.published.borrow()));
    }
    fn event(&mut self, ctx: &mut crate::event::EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                EventResult::Handled
            }
            InputEvent::Key(_) => {
                ctx.release_focus();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

struct PaintImeView {
    published: Rc<RefCell<String>>,
}

impl View<ClickState> for PaintImeView {
    type Element = PaintImeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PaintImeWidget {
        PaintImeWidget {
            published: self.published.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut PaintImeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

fn key_event() -> InputEvent {
    InputEvent::Key(crate::event::KeyEvent {
        key: crate::event::Key::Named(crate::event::NamedKey::Enter),
        modifiers: crate::event::Modifiers::default(),
        repeat: false,
    })
}

#[test]
fn focus_ime_generation_moves_on_every_pointer_transition_only() {
    let mut root: RenderRoot<ClickState, ImeView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut ime_logic, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Idle: a rebuild/layout touches neither focus nor the IME surface.
    let idle = root.focus_ime_generation();
    root.rebuild(&mut ime_logic, &mut state);
    assert_eq!(
        root.focus_ime_generation(),
        idle,
        "a rebuild is not an edge"
    );

    // Focus gained + IME surface published (one transition for a shell,
    // however many field writes it took).
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    let focused = root.focus_ime_generation();
    assert_ne!(
        focused, idle,
        "focus + IME publish must move the generation"
    );

    // The SAME tap again, on the already-focused widget publishing the
    // identical surface: no state moved, so no edge. This is the case that
    // decides whether a live text field forces a frame per vsync.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    assert_eq!(
        root.focus_ime_generation(),
        focused,
        "a same-value focus/IME write must not spin the edge"
    );

    // Blur: focus cleared and the surface dropped — a real transition.
    root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 10.0));
    let blurred = root.focus_ime_generation();
    assert_ne!(blurred, focused, "a blur must move the generation");

    // Blur while already blurred (a tap on inert chrome — the commonest
    // event there is) writes `false`/`None` back over `false`/`None`.
    root.event(&mut state, &pointer(PointerPhase::Down, 80.0, 20.0));
    assert_eq!(
        root.focus_ime_generation(),
        blurred,
        "blurring an already-blurred root must not move the generation"
    );
}

#[test]
fn focus_ime_generation_ignores_an_unchanged_paint_republish() {
    // The paint pass re-publishes the focused widget's IME surface on EVERY
    // frame (that is how a rebuild-applied controlled change refreshes the
    // shell-facing state). If that unconditional write moved the
    // generation, the frame gate's edge would fire every single frame for
    // the whole life of a focus session — exactly the per-vsync forcing the
    // edge exists to remove.
    let published = Rc::new(RefCell::new("abc".to_string()));
    let mut build = {
        let published = published.clone();
        move |_state: &mut ClickState| PaintImeView {
            published: published.clone(),
        }
    };
    let mut root: RenderRoot<ClickState, PaintImeView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut build, &mut state);
    root.layout(Size::new(100.0, 100.0));

    // Focus the field, then let it paint: the first paint publishes.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    let steady = root.focus_ime_generation();
    assert_eq!(
        root.ime_state(),
        Some(PaintImeWidget::surface("abc")),
        "the paint pass published the focused widget's surface"
    );

    // 120 further frames of the same focused, unchanged field: the caret
    // blinks, nothing else moves. Not one edge.
    for _ in 0..120 {
        root.paint(&mut scene, FrameTime::ZERO);
    }
    assert_eq!(
        root.focus_ime_generation(),
        steady,
        "an unchanged paint republish must never move the generation"
    );

    // A real change (the app applied a controlled edit) publishes a
    // different surface: exactly one edge, then quiet again.
    *published.borrow_mut() = "abcd".to_string();
    root.paint(&mut scene, FrameTime::ZERO);
    let edited = root.focus_ime_generation();
    assert_ne!(edited, steady, "a changed republish IS an edge");
    for _ in 0..10 {
        root.paint(&mut scene, FrameTime::ZERO);
    }
    assert_eq!(
        root.focus_ime_generation(),
        edited,
        "the session goes quiet again at the new value"
    );
}

#[test]
fn focus_ime_generation_moves_on_a_focus_release_only_once() {
    // The Key/Ime/Scroll arm of the root focus path: a dispatch that
    // RELEASES focus clears both the flag and the published surface.
    let published = Rc::new(RefCell::new("abc".to_string()));
    let mut build = {
        let published = published.clone();
        move |_state: &mut ClickState| PaintImeView {
            published: published.clone(),
        }
    };
    let mut root: RenderRoot<ClickState, PaintImeView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut build, &mut state);
    root.layout(Size::new(100.0, 100.0));
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    let focused = root.focus_ime_generation();
    assert!(root.is_focus_active());

    // A key that releases focus: one edge.
    root.event(&mut state, &key_event());
    let released = root.focus_ime_generation();
    assert!(!root.is_focus_active());
    assert!(root.ime_state().is_none());
    assert_ne!(
        released, focused,
        "a focus release must move the generation"
    );

    // A second release against an already-released root: no edge. (The
    // paint pass republishes nothing now — the paint-take arm only accepts
    // a publish while focus is active.)
    root.event(&mut state, &key_event());
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(
        root.focus_ime_generation(),
        released,
        "releasing an already-released focus must not move the generation"
    );
}

// --- The focus session's IDENTITY, beside the edge generation ------------
//
// `focus_epoch` answers a question `focus_ime_generation` cannot: "is this
// still the session that asked?". A caller that binds a slow, asynchronous
// answer to the field that asked for it needs an identity, and a counter
// over the published surface's *value* is not one — two fields publish
// equal surfaces, and a field is free to take focus and publish nothing at
// all.
//
// Each test below asserts what BOTH counters did at the same moment. The
// `focus_ime_generation` assertions are the point rather than decoration:
// they are what states, in a form the compiler checks, that the edge
// generation stands still exactly where the identity moves.

/// The text both fields publish from a press, so neither can be told from
/// the other by the published value alone.
const SHARED_FIELD_TEXT: &str = "shared";

/// One field of the two-field fixture.
///
/// Takes the focus session on any press inside itself; publishes an IME
/// surface on that press only when built to; and treats an `Ime` event as an
/// edit — the text changes and the surface is republished, but nothing
/// re-claims a session the field already holds.
///
/// `publishes: false` is not a contrivance: the baseline text input claims
/// focus and republishes nothing for a press that lands inside text it
/// already had selected (such a press moves no caret and collapses no
/// selection), and any app-authored focusable that publishes no IME surface
/// of its own behaves the same way.
///
/// A press publishes the *shared* surface, so the two fields are
/// indistinguishable to anything reading the published value — that is the
/// case under test. An edit publishes the field's own `name` instead, which
/// is how a test proves which field a focus-routed event actually reached.
struct SessionField {
    name: &'static str,
    publishes: bool,
}

impl SessionField {
    /// The surface a field publishes. Deliberately carries nothing that
    /// tells one field from another: `ImeState` is
    /// `{active, editing, caret, content_type}` and names no widget, so two
    /// fields holding the same text and caret publish equal values — which
    /// is the ordinary shape of two empty fields, or two overlapping ones
    /// mid-transition.
    fn surface(text: &str) -> ImeState {
        ImeState {
            active: true,
            editing: EditingState {
                text: text.to_string(),
                selection_base: 0,
                selection_extent: 0,
                composing_base: -1,
                composing_extent: -1,
            },
            caret: Some(kurbo::Rect::new(0.0, 0.0, 1.0, 12.0)),
            content_type: Default::default(),
            suppress_soft_keyboard: false,
        }
    }
}

impl crate::widget::Widget for SessionField {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(100.0, 20.0))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                ctx.request_focus();
                if self.publishes {
                    ctx.publish_ime_state(Self::surface(SHARED_FIELD_TEXT));
                }
                EventResult::Handled
            }
            InputEvent::Ime(_) => {
                ctx.publish_ime_state(Self::surface(self.name));
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

/// The fixture's root: two stacked fields, with a pointer event hit-tested
/// to the one under it and a focus-routed event forwarded down the recorded
/// focus path without a hit test — the routing every real container does.
struct TwoFields {
    top: crate::widget::ChildPod,
    bottom: crate::widget::ChildPod,
}

impl crate::widget::Widget for TwoFields {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.top.layout_child(ctx, bc);
        self.top.set_origin(Point::new(0.0, 0.0));
        self.bottom.layout_child(ctx, bc);
        self.bottom.set_origin(Point::new(0.0, 50.0));
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.top.paint_child(ctx, scene);
        self.bottom.paint_child(ctx, scene);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if matches!(event, InputEvent::Pointer(_)) {
            let pos = event.position();
            if self.top.contains(pos) {
                return self.top.event_child(ctx, event);
            }
            if self.bottom.contains(pos) {
                return self.bottom.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if self.top.holds_live_focus() {
            return self.top.event_child(ctx, event);
        }
        if self.bottom.holds_live_focus() {
            return self.bottom.event_child(ctx, event);
        }
        EventResult::Ignored
    }
    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.top.semantics_child(ctx);
        self.bottom.semantics_child(ctx);
    }
}

struct TwoFieldsView {
    bottom_publishes: bool,
}

impl View<ClickState> for TwoFieldsView {
    type Element = TwoFields;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TwoFields {
        TwoFields {
            top: crate::widget::ChildPod::new(Box::new(SessionField {
                name: "top",
                publishes: true,
            })),
            bottom: crate::widget::ChildPod::new(Box::new(SessionField {
                name: "bottom",
                publishes: self.bottom_publishes,
            })),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut TwoFields,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

/// An edit pushed down the focus path by the platform IME: it claims no
/// focus, so only the field already holding the session sees it — which is
/// what makes the surface it republishes name that field.
fn edit_event() -> InputEvent {
    InputEvent::Ime(crate::event::ImeEvent::ApplyEditingState(
        SessionField::surface(SHARED_FIELD_TEXT).editing,
    ))
}

/// Mount the two-field fixture and press the top field, returning the root
/// with a live session on it.
fn two_fields_focused(
    bottom_publishes: bool,
) -> (RenderRoot<ClickState, TwoFieldsView>, ClickState) {
    let mut root: RenderRoot<ClickState, TwoFieldsView> = RenderRoot::new();
    let mut state = ClickState::default();
    root.rebuild(&mut |_| TwoFieldsView { bottom_publishes }, &mut state);
    root.layout(Size::new(200.0, 200.0));
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
    assert!(root.is_focus_active(), "the top field opened a session");
    (root, state)
}

#[test]
fn focus_epoch_moves_when_focus_crosses_two_fields_publishing_alike() {
    let (mut root, mut state) = two_fields_focused(true);
    let first_session = root.focus_epoch();
    let steady_edge = root.focus_ime_generation();
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface(SHARED_FIELD_TEXT))
    );

    // Press the bottom field. Focus really does move — and nothing
    // observable about the published surface moves with it: claiming while
    // some field is already focused writes `true` over `true`, and the
    // surface the second field publishes compares equal to the first's.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 60.0));
    assert!(root.is_focus_active());
    assert_eq!(
        root.focus_ime_generation(),
        steady_edge,
        "the edge generation is blind to this move, which is why a caller \
         asking 'is this the same session?' must not be built on it"
    );
    assert_ne!(
        root.focus_epoch(),
        first_session,
        "the session identity must move when focus crosses to another field"
    );

    // Not merely "some counter moved": the focus PATH is the bottom
    // field's now, which a focus-routed event proves by reaching it.
    root.event(&mut state, &edit_event());
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface("bottom")),
        "the second field is the one holding the session"
    );
}

#[test]
fn focus_epoch_moves_when_the_field_taking_focus_publishes_nothing() {
    let (mut root, mut state) = two_fields_focused(false);
    let first_session = root.focus_epoch();
    let steady_edge = root.focus_ime_generation();

    // Press the bottom field, which takes the session and publishes no
    // surface of its own. A publish-nothing dispatch leaves the standing
    // surface standing, so what the shell still sees describes the field
    // the user just left — for as long as this session lasts, not merely
    // until the next frame.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 60.0));
    assert!(root.is_focus_active());
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface(SHARED_FIELD_TEXT)),
        "the field that lost focus is still the one the surface describes"
    );
    assert_eq!(
        root.focus_ime_generation(),
        steady_edge,
        "no published value moved, so the edge generation cannot have"
    );
    assert_ne!(
        root.focus_epoch(),
        first_session,
        "an honoured claim moves the session identity whether or not the \
         claimant publishes anything"
    );

    // And again, the move is a real one: the focus path now ends at the
    // field that published nothing.
    root.event(&mut state, &edit_event());
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface("bottom")),
        "the second field is the one holding the session"
    );
}

#[test]
fn focus_epoch_ignores_an_edit_inside_one_session() {
    // The converse direction, and the reason the two counters are kept
    // apart rather than merged: a caller holding an identity can let a
    // harmless edit ride, where a caller comparing the published value has
    // to treat every keystroke as a reason to give up.
    let (mut root, mut state) = two_fields_focused(true);
    let session = root.focus_epoch();
    let before_edit = root.focus_ime_generation();

    root.event(&mut state, &edit_event());
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface("top")),
        "the focused field applied the edit and republished"
    );
    assert_ne!(
        root.focus_ime_generation(),
        before_edit,
        "a changed surface IS an edge"
    );
    assert_eq!(
        root.focus_epoch(),
        session,
        "an edit does not end or restart the session it lands in"
    );

    // A release ends the identity too, so a stale one can never come back
    // round to matching by standing still.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 90.0));
    assert!(!root.is_focus_active());
    assert_ne!(
        root.focus_epoch(),
        session,
        "a release retires the session's identity"
    );
}
