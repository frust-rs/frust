//! Focus, IME surface and focus-session identity tests for RenderRoot.

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

// --- Session release: the focus session must die with its owner ---
//
// A session can end without input reaching the root: a widget publishes an INACTIVE
// IME surface, or a reconciler unmounts the focused pod (raising `mark_focus_orphaned`).
// Both must perform the same full release a blur does.

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

    // The widget keeps publishing the cleared surface every frame; the paint take's
    // `focus_active` guard makes each a no-op, so the session neither resurrects nor spins.
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
    // (b) The child-list-diff shape: no publish, no event; the focused widget
    // just stops existing, so only the reconciler's mark can release it.
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

// --- The focus/IME EDGE generation ---
//
// `focus_ime_generation` must move on EVERY real transition and on NO same-value write,
// or a focused screen forces a frame every vsync.

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

    // The same tap on the already-focused widget publishing an identical surface:
    // no state moved, so no edge.
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
    // Paint re-publishes the IME surface every frame; if that write moved the
    // generation, the edge would fire per frame for the whole focus session.
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

    // A second release against an already-released root: no edge (paint only
    // accepts a publish while focus is active).
    root.event(&mut state, &key_event());
    root.paint(&mut scene, FrameTime::ZERO);
    assert_eq!(
        root.focus_ime_generation(),
        released,
        "releasing an already-released focus must not move the generation"
    );
}

// --- The focus session's IDENTITY, beside the edge generation ---
//
// `focus_epoch` answers "is this still the session that asked?", which a counter over
// the published surface's value cannot: two fields publish equal surfaces, and a field
// may take focus and publish nothing. Each test asserts what both counters did.

/// The text both fields publish from a press, so neither can be told from
/// the other by the published value alone.
const SHARED_FIELD_TEXT: &str = "shared";

/// One field of the two-field fixture.
///
/// A press takes the focus session and, only if `publishes`, publishes the shared
/// surface (a text input pressed inside already-selected text claims focus and
/// publishes nothing). An `Ime` edit republishes the field's own `name`, which
/// proves which field a focus-routed event reached.
struct SessionField {
    name: &'static str,
    publishes: bool,
}

impl SessionField {
    /// The surface a field publishes; it carries nothing that tells fields apart, so
    /// two fields with the same text and caret publish equal values.
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

    // Press the bottom field: focus moves, but nothing observable about the
    // published surface does (it compares equal to the first field's).
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

    // The bottom field takes the session and publishes nothing: the surface of the
    // field the user left is dropped, and the edge generation moves with it.
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 60.0));
    assert!(root.is_focus_active());
    assert_eq!(root.ime_state(), None, "the left field's surface is gone");
    assert_ne!(root.focus_ime_generation(), steady_edge);
    assert_ne!(
        root.focus_epoch(),
        first_session,
        "an honoured claim moves the session identity whether or not the \
         claimant publishes anything"
    );

    // The move is a real one: the focus path now ends at the field that
    // published nothing.
    root.event(&mut state, &edit_event());
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface("bottom")),
        "the second field is the one holding the session"
    );
}

#[test]
fn focus_epoch_ignores_an_edit_inside_one_session() {
    // The converse, and why the counters stay apart: an identity holder can let a
    // harmless edit ride, while a value comparer must give up on every keystroke.
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

#[test]
fn pointer_focus_move_to_a_non_publishing_pod_clears_the_surface_at_paint() {
    let (mut root, mut state) = two_fields_focused(false);
    let first_session = root.focus_epoch();
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(root.ime_state().is_some(), "the top field's surface stands");

    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 60.0));
    let moved = root.focus_epoch();
    assert_ne!(moved, first_session);
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(root.is_focus_active(), "the bottom pod holds the session");
    assert!(root.ime_state().is_none(), "no stale surface after paint");
    assert_eq!(root.focus_epoch(), moved, "the epoch moved exactly once");
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(root.ime_state().is_none());
}

#[test]
fn pointer_focus_move_between_publishing_pods_keeps_the_new_surface() {
    let (mut root, mut state) = two_fields_focused(true);
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 60.0));
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(root.is_focus_active());
    assert_eq!(
        root.ime_state(),
        Some(SessionField::surface(SHARED_FIELD_TEXT))
    );
}

#[test]
fn outside_tap_blur_still_clears_the_surface() {
    let (mut root, mut state) = two_fields_focused(true);
    let mut scene = RecordingScene::default();
    root.paint(&mut scene, FrameTime::ZERO);
    root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 90.0));
    root.paint(&mut scene, FrameTime::ZERO);
    assert!(!root.is_focus_active());
    assert!(root.ime_state().is_none());
}
