//! Fixtures shared by more than one `RenderRoot` test family (overlay, IME and focus
//! harnesses), plus the declarations of the family modules.
mod clipboard;
/// The root half of the multi-contact contract (`InputEvent::PointerContact`):
/// how a contact's id, the claimant latch and the `capture_contacts` opt-in
/// decide where an event goes.
mod contacts;
mod cursor;
/// The [`RenderRoot::event`] file-drag follow-up: every `Drop`/`Cancel`,
/// handled or not, is followed by an `Ended` broadcast.
mod file_drop;
mod focus_ime;
mod hover;
mod lifecycle;
mod overlay;
mod platform_views;
/// [`InputEvent::Scale`]: hit-tested by [`ScaleEvent::focal`] exactly like
/// [`InputEvent::Scroll`], translated down the tree the same way, and bubbles
/// topmost-first until a widget reports [`EventResult::Handled`].
mod scale;
mod selection_toolbar;
mod semantics;

use super::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use kurbo::{Point, Rect, Size};

use crate::anim::FrameTime;
use crate::event::{
    CursorIcon, EditCommand, EventCtx, EventOutcome, EventResult, FileDropEvent, FileDropPhase,
    ImeContentType, ImeState, InputEvent, Key, KeyEvent, Modifiers, NamedKey, OverlayEventKind,
    PointerButton, PointerEvent, PointerId, PointerPhase, ScrollDelta,
};
use crate::layout::BoxConstraints;
use crate::overlay::{OutsideTap, OverlayEntry, OverlayInput, OverlayKey};
use crate::semantics::{ROOT_NODE_ID, SemanticsCtx, SemanticsUpdate};
use crate::view::{BuildCtx, ChangeFlags, View};
use crate::widget::{LayoutCtx, PaintCtx, PaintScene, PlatformViewFrame};

/// Application state for the tests.
#[derive(Default)]
pub(super) struct AppState {
    label: String,
}

/// The retained widget produced by `MockTextView`: stores the current text
/// and records what it painted.
pub(super) struct TextWidget {
    text: String,
}

impl crate::widget::Widget for TextWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // A crude intrinsic size: width proportional to text length.
        let intrinsic = Size::new(self.text.len() as f64 * 8.0, 16.0);
        bc.constrain(intrinsic)
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.draw_text(ctx.origin(), &self.text);
    }
}

/// The task's `MockTextView`: a real `View` impl living in tests.
pub(super) struct MockTextView {
    text: String,
}

impl View<AppState> for MockTextView {
    type Element = TextWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
        TextWidget {
            text: self.text.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.text != self.text {
            element.text = self.text.clone();
            // Text change: same size model would relayout, but the intrinsic
            // width can change, so signal PAINT here and let callers decide.
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// A scene recorder for asserting paint output.
#[derive(Default)]
pub(super) struct RecordingScene {
    texts: Vec<(Point, String)>,
}

impl PaintScene for RecordingScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: peniko::Color) {}
    fn draw_text(&mut self, origin: Point, text: &str) {
        self.texts.push((origin, text.to_string()));
    }
    fn draw_scene_texture(&mut self, _id: u64, _dest: Rect) {}
}

pub(super) fn build(state: &mut AppState) -> MockTextView {
    MockTextView {
        text: state.label.clone(),
    }
}

#[derive(Default)]
pub(super) struct ClickState {
    clicks: u32,
}

pub(super) fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(crate::event::PointerEvent {
        phase,
        position: Point::new(x, y),
        button: crate::event::PointerButton::Primary,
    })
}

/// Which of the fixture's two leaves an assertion is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Leaf {
    Top,
    Bottom,
}

/// What the owner does when a `Down` arrives inside one of its surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OwnerReaction {
    /// Nothing — the commonest shape (a menu item acts on `Up`).
    Nothing,
    /// Capture the pointer, the drag-from-inside-a-surface shape.
    Capture,
    /// Claim focus, the text-field-inside-a-popover shape.
    Focus,
}

/// One floated surface the fixture's owner registers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SurfaceSpec {
    key: OverlayKey,
    label: &'static str,
    rect: Rect,
    band: crate::overlay::OverlayBand,
    input: OverlayInput,
    outside_tap: OutsideTap,
    /// Whether the owner registers it at all this frame — the fixture for
    /// "stop registering and the surface stops existing".
    register: bool,
    on_down: OwnerReaction,
    /// The same, for a `Move` inside the surface — a drag threshold latching
    /// a capture mid-gesture, the shape that claims one on no `Down` at all.
    on_move: OwnerReaction,
    /// The same again, for an `Up` — the phase on which a capture claim has
    /// nothing left to own, and the one that reaches the surface as an
    /// overlay event precisely because no capture was standing to divert it.
    on_up: OwnerReaction,
    /// Whether the leaf INSIDE the pod claims focus on a `Down` of its own —
    /// the editable-in-a-popover shape, distinct from `on_down`'s claim,
    /// which the owner makes on the surface's behalf.
    pod_claims_focus: bool,
    /// Whether that leaf republishes an active IME surface on every paint.
    /// Stated unconditionally, so the fixture can publish from a pod holding
    /// no focus link at all — the provenance case.
    pod_publishes_ime: bool,
    /// The same publish, from the leaf's `event` handler instead of its
    /// paint — the event-route half of the provenance case, and equally
    /// ungated.
    pod_publishes_ime_on_event: bool,
}

impl SurfaceSpec {
    fn floating(label: &'static str, rect: Rect) -> Self {
        Self {
            key: OverlayKey::next(),
            label,
            rect,
            band: crate::overlay::OverlayBand::Floating,
            input: OverlayInput::Interactive,
            outside_tap: OutsideTap::Ignore,
            register: true,
            on_down: OwnerReaction::Nothing,
            on_move: OwnerReaction::Nothing,
            on_up: OwnerReaction::Nothing,
            pod_claims_focus: false,
            pod_publishes_ime: false,
            pod_publishes_ime_on_event: false,
        }
    }
}

/// Everything the overlay fixture recorded, shared between the widgets and
/// the test.
#[derive(Default)]
pub(super) struct OverlayLog {
    /// Events the owner received, in the order they arrived.
    owner: Vec<InputEvent>,
    /// `(surface label, event)` for everything a floated pod's content saw —
    /// positions here are the pod's own local space.
    pod: Vec<(&'static str, InputEvent)>,
    /// Events the main-tree sibling received.
    sibling: Vec<InputEvent>,
    /// What the sibling read from `EventCtx::has_focus` on each event.
    sibling_focus: Vec<bool>,
    /// What the pod's leaf read from `PaintCtx::has_focus` on each paint.
    pod_paint_focus: Vec<bool>,
    /// The same read for the main-tree sibling. The two together answer
    /// "which branches believe they are focused".
    sibling_paint_focus: Vec<bool>,
    /// The window size the owner observed in its own (child) layout.
    child_window_size: Option<Size>,
}

pub(super) type Log = std::rc::Rc<std::cell::RefCell<OverlayLog>>;

/// The leaf inside a floated pod: paints its label at the pod's absolute origin and
/// records what reaches it. Optionally an editable that claims focus on `Down` and
/// republishes an IME surface on every paint, deliberately ungated.
pub(super) struct OverlayContentWidget {
    label: &'static str,
    log: Log,
    claims_focus: bool,
    publishes_ime: bool,
    publishes_ime_on_event: bool,
}

impl OverlayContentWidget {
    /// What such a pod publishes: an ordinary field's surface, carrying no
    /// secure-text configuration of its own.
    fn surface() -> ImeState {
        ImeState {
            active: true,
            content_type: ImeContentType::Normal,
            ..Default::default()
        }
    }
}

impl crate::widget::Widget for OverlayContentWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(80.0, 30.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.draw_text(ctx.origin(), self.label);
        self.log.borrow_mut().pod_paint_focus.push(ctx.has_focus());
        if self.publishes_ime {
            ctx.publish_ime_state(Self::surface());
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.log.borrow_mut().pod.push((self.label, event.clone()));
        if self.publishes_ime_on_event {
            ctx.publish_ime_state(Self::surface());
        }
        if self.claims_focus
            && let InputEvent::Pointer(pointer) = event
            && pointer.phase == PointerPhase::Down
        {
            ctx.request_focus();
        }
        EventResult::Handled
    }
}

/// One live surface: its spec plus the pod the owner keeps.
pub(super) struct OwnedSurface {
    spec: SurfaceSpec,
    pod: crate::overlay::OverlayPod,
}

/// The overlay owner: hosts the pods, registers them from `paint`, and
/// forwards the broadcasts the root routes back to it into the right pod.
pub(super) struct OverlayOwnerWidget {
    surfaces: Vec<OwnedSurface>,
    log: Log,
}

impl OverlayOwnerWidget {
    fn new(specs: &[SurfaceSpec], log: Log) -> Self {
        let mut owner = Self {
            surfaces: Vec::new(),
            log,
        };
        owner.sync(specs);
        owner
    }

    /// Reconcile the live surfaces against `specs`, keeping each pod alive
    /// across a rebuild (a real owner keeps its popover's widget state).
    fn sync(&mut self, specs: &[SurfaceSpec]) {
        self.surfaces
            .retain(|s| specs.iter().any(|n| n.key == s.spec.key));
        for spec in specs {
            match self.surfaces.iter_mut().find(|s| s.spec.key == spec.key) {
                Some(live) => live.spec = spec.clone(),
                None => {
                    let log = std::rc::Rc::clone(&self.log);
                    self.surfaces.push(OwnedSurface {
                        spec: spec.clone(),
                        pod: std::rc::Rc::new(std::cell::RefCell::new(
                            crate::widget::ChildPod::new(Box::new(OverlayContentWidget {
                                label: spec.label,
                                log,
                                claims_focus: spec.pod_claims_focus,
                                publishes_ime: spec.pod_publishes_ime,
                                publishes_ime_on_event: spec.pod_publishes_ime_on_event,
                            })),
                        )),
                    });
                }
            }
        }
    }
}

impl crate::widget::Widget for OverlayOwnerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The window size reaches a CHILD's layout unchanged — this widget is
        // a `ChildPod` of the fixture's root.
        self.log.borrow_mut().child_window_size = Some(ctx.window_size());
        // A floated pod is laid out against the WINDOW, never against the
        // owner's own constraints: it escapes the owner's box entirely.
        let window = BoxConstraints::loose(ctx.window_size());
        for surface in &mut self.surfaces {
            surface.pod.borrow_mut().layout_child(ctx, &window);
        }
        bc.constrain(Size::new(40.0, 20.0))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.draw_text(ctx.origin(), "owner");
        for surface in &self.surfaces {
            if !surface.spec.register {
                continue;
            }
            // Registered, never painted here: the root paints it last.
            ctx.register_overlay(OverlayEntry {
                key: surface.spec.key,
                band: surface.spec.band,
                input: surface.spec.input,
                outside_tap: surface.spec.outside_tap,
                window_rect: surface.spec.rect,
                pod: std::rc::Rc::clone(&surface.pod),
                insets: ctx.window_insets(),
            });
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Overlay(overlay) = event else {
            self.log.borrow_mut().owner.push(event.clone());
            return EventResult::Ignored;
        };
        let Some(surface) = self.surfaces.iter_mut().find(|s| s.spec.key == overlay.key) else {
            // Another owner's surface: the broadcast reached us, and we
            // ignore it. This fall-through is the whole addressing rule.
            return EventResult::Ignored;
        };
        self.log.borrow_mut().owner.push(event.clone());
        match &overlay.kind {
            OverlayEventKind::Pointer(pointer) => {
                let reaction = match pointer.phase {
                    PointerPhase::Down => Some(surface.spec.on_down),
                    PointerPhase::Move => Some(surface.spec.on_move),
                    PointerPhase::Up => Some(surface.spec.on_up),
                    _ => None,
                };
                match reaction {
                    Some(OwnerReaction::Capture) => ctx.capture_pointer(),
                    Some(OwnerReaction::Focus) => ctx.request_focus(),
                    Some(OwnerReaction::Nothing) | None => {}
                }
                // Window space → the pod's own space is one subtraction: the
                // registered rect's origin.
                let local = InputEvent::Pointer(PointerEvent {
                    position: pointer.position - surface.spec.rect.origin().to_vec2(),
                    ..*pointer
                });
                surface.pod.borrow_mut().event_child(ctx, &local);
            }
            OverlayEventKind::Scroll { position, delta } => {
                let local = InputEvent::Scroll {
                    position: *position - surface.spec.rect.origin().to_vec2(),
                    delta: *delta,
                };
                surface.pod.borrow_mut().event_child(ctx, &local);
            }
            OverlayEventKind::Scale {
                focal,
                phase,
                scale_delta,
                velocity,
            } => {
                let local = InputEvent::Scale(crate::event::ScaleEvent {
                    phase: *phase,
                    scale_delta: *scale_delta,
                    focal: *focal - surface.spec.rect.origin().to_vec2(),
                    velocity: *velocity,
                });
                surface.pod.borrow_mut().event_child(ctx, &local);
            }
            OverlayEventKind::OutsideDown => {}
        }
        // A broadcast is never consumed, whatever the pod returned.
        EventResult::Ignored
    }
}

/// The main-tree sibling: painted AFTER the owner, claims focus on a `Down` inside
/// it, and records what it sees. It stands in for a secure text field, republishing
/// an [`ImeContentType::Password`] surface on every paint while seeded focused.
pub(super) struct SiblingLeafWidget {
    log: Log,
    /// The selection-toolbar request to publish each paint, if any.
    publish: Option<crate::selection_toolbar::SelectionToolbarRequest>,
}

impl SiblingLeafWidget {
    /// The secure surface this field describes its session with.
    fn surface() -> ImeState {
        ImeState {
            active: true,
            content_type: ImeContentType::Password,
            ..Default::default()
        }
    }
}

impl crate::widget::Widget for SiblingLeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(60.0, 40.0))
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.draw_text(ctx.origin(), "sibling");
        self.log
            .borrow_mut()
            .sibling_paint_focus
            .push(ctx.has_focus());
        if ctx.has_focus() {
            ctx.publish_ime_state(Self::surface());
        }
        if let Some(request) = self.publish {
            ctx.publish_selection_toolbar(request);
        }
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        {
            let mut log = self.log.borrow_mut();
            log.sibling.push(event.clone());
            log.sibling_focus.push(ctx.has_focus());
        }
        match event {
            InputEvent::Pointer(pointer) if pointer.phase == PointerPhase::Down => {
                ctx.request_focus();
                ctx.publish_ime_state(Self::surface());
                EventResult::Handled
            }
            // A scroll is hit-tested like a press but travels the keyboard-class
            // focus arm, so claiming here is a focus move that is not a `Down`.
            InputEvent::Scroll { .. } => {
                ctx.request_focus();
                ctx.publish_ime_state(Self::surface());
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }
}

/// The fixture's root: a hand-written two-child container routing exactly
/// like `frust-widgets`' helpers — broadcast first, then capture, then focus,
/// then a topmost-first hit test.
pub(super) struct OverlayRootWidget {
    owner: crate::widget::ChildPod,
    sibling: crate::widget::ChildPod,
}

impl crate::widget::Widget for OverlayRootWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.owner.layout_child(ctx, bc);
        self.owner.set_origin(Point::new(0.0, 0.0));
        self.sibling.layout_child(ctx, bc);
        self.sibling.set_origin(Point::new(0.0, 100.0));
        bc.constrain(Size::new(200.0, 200.0))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The owner paints FIRST, the sibling after it — so an overlay pod
        // landing after both proves the root's post-pass really is last.
        self.owner.paint_child(ctx, scene);
        self.sibling.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.owner.event_child(ctx, event);
            self.sibling.event_child(ctx, event);
            return EventResult::Ignored;
        }
        let ends = matches!(
            event,
            InputEvent::Pointer(p)
                if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
        );
        for (pod, _) in [(&mut self.owner, 0), (&mut self.sibling, 1)] {
            if pod.is_active() {
                let result = pod.event_child(ctx, event);
                if ends {
                    pod.set_active(false);
                }
                return result;
            }
        }
        if event.is_focus_routed() {
            // `holds_live_focus`, not `is_focused` (as `frust-widgets`' `route_event`):
            // both children can carry a recorded link, only one the live session's.
            if self.sibling.holds_live_focus() {
                return self.sibling.event_child(ctx, event);
            }
            if self.owner.holds_live_focus() {
                return self.owner.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // Topmost-first: the sibling paints last, so it hit-tests first.
        let position = event.position();
        if self.sibling.contains(position) {
            return self.sibling.event_child(ctx, event);
        }
        if self.owner.contains(position) {
            return self.owner.event_child(ctx, event);
        }
        EventResult::Ignored
    }
}

/// The view producing the fixture, reconciling the surface specs in place.
pub(super) struct OverlayRootView {
    specs: Vec<SurfaceSpec>,
    publish: Option<crate::selection_toolbar::SelectionToolbarRequest>,
    log: Log,
}

impl View<OverlayState> for OverlayRootView {
    type Element = OverlayRootWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> Self::Element {
        OverlayRootWidget {
            owner: crate::widget::ChildPod::new(Box::new(OverlayOwnerWidget::new(
                &self.specs,
                std::rc::Rc::clone(&self.log),
            ))),
            sibling: crate::widget::ChildPod::new(Box::new(SiblingLeafWidget {
                log: std::rc::Rc::clone(&self.log),
                publish: self.publish,
            })),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element
            .owner
            .widget_mut()
            .downcast_mut::<OverlayOwnerWidget>()
            .expect("the owner keeps its type")
            .sync(&self.specs);
        element
            .sibling
            .widget_mut()
            .downcast_mut::<SiblingLeafWidget>()
            .expect("the sibling keeps its type")
            .publish = self.publish;
        if prev.specs != self.specs || prev.publish != self.publish {
            ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }
}

/// App state for the overlay fixture: the surface specs the next rebuild
/// applies, plus the shared log.
pub(super) struct OverlayState {
    specs: Vec<SurfaceSpec>,
    publish: Option<crate::selection_toolbar::SelectionToolbarRequest>,
    log: Log,
}

pub(super) fn overlay_logic(state: &mut OverlayState) -> OverlayRootView {
    OverlayRootView {
        specs: state.specs.clone(),
        publish: state.publish,
        log: std::rc::Rc::clone(&state.log),
    }
}

/// Drives the overlay fixture the way a shell does: rebuild, layout, paint,
/// then dispatch.
pub(super) struct OverlayHarness {
    root: RenderRoot<OverlayState, OverlayRootView>,
    state: OverlayState,
    log: Log,
}

impl OverlayHarness {
    fn new(specs: Vec<SurfaceSpec>) -> Self {
        let log: Log = std::rc::Rc::new(std::cell::RefCell::new(OverlayLog::default()));
        let mut harness = Self {
            root: RenderRoot::new(),
            state: OverlayState {
                specs,
                publish: None,
                log: std::rc::Rc::clone(&log),
            },
            log,
        };
        harness.frame();
        harness
    }

    /// One full frame: rebuild, layout at a 200x200 window, paint.
    fn frame(&mut self) -> RecordingScene {
        self.root.rebuild(&mut overlay_logic, &mut self.state);
        self.root.layout(Size::new(200.0, 200.0));
        let mut scene = RecordingScene::default();
        self.root.paint(&mut scene, FrameTime::ZERO);
        scene
    }

    fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
        self.root.event(&mut self.state, event)
    }

    fn down(&mut self, x: f64, y: f64) -> EventOutcome {
        self.dispatch(&pointer(PointerPhase::Down, x, y))
    }

    fn clear_log(&mut self) {
        let mut log = self.log.borrow_mut();
        log.owner.clear();
        log.pod.clear();
        log.sibling.clear();
        log.sibling_focus.clear();
        log.pod_paint_focus.clear();
        log.sibling_paint_focus.clear();
    }

    /// What the pod's leaf and the main-tree sibling each read from
    /// `PaintCtx::has_focus` on the most recent frame — "which branches
    /// believe they are focused", read from the branches themselves.
    fn branch_focus(&self) -> (bool, bool) {
        let log = self.log.borrow();
        (
            *log.pod_paint_focus
                .last()
                .expect("the pod painted at least once"),
            *log.sibling_paint_focus
                .last()
                .expect("the sibling painted at least once"),
        )
    }

    /// The overlay events the owner received, as `(key, kind)`.
    fn owner_overlays(&self) -> Vec<(OverlayKey, OverlayEventKind)> {
        self.log
            .borrow()
            .owner
            .iter()
            .filter_map(|event| match event {
                InputEvent::Overlay(o) => Some((o.key, o.kind.clone())),
                _ => None,
            })
            .collect()
    }

    /// Pointer events the sibling saw — the "did the main tree get it?" read.
    fn sibling_pointers(&self) -> Vec<PointerEvent> {
        self.log
            .borrow()
            .sibling
            .iter()
            .filter_map(|event| match event {
                InputEvent::Pointer(p) => Some(*p),
                _ => None,
            })
            .collect()
    }
}
