//! Shared test fixtures for `navigator_tests`' themed child modules: pointer
//! helpers, retained-state probe widgets (`CounterView`/`SizedLeaf`/`HostProbe`),
//! frame-driving helpers (`ft`/`full_frame`/`fill_h`/`run_until_settled`), and
//! small state carriers (`ResultState`/`SwipeResultState`/`AppLogic`) used by
//! more than one theme. Items here are `pub(super)` — visible to `tests` and
//! every sibling theme module, never further.

use super::super::*;
use crate::Stack;
use frust_core::{FrameTime, PointerButton, PointerEvent, PointerPhase, RenderRoot, any};
use std::any::Any;
use std::cell::Cell;

pub(super) fn down(x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Down,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

pub(super) fn move_to(x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Move,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

pub(super) fn up(x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Up,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

pub(super) fn cancel_ev(x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Cancel,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

// --- A leaf with retained internal state (a counter) that reports its value
//     during paint, so a test can prove the page's widget state survives a
//     push→pop round-trip (pod retention, not a rebuild-from-scratch). ---
struct CounterView {
    observed: Rc<Cell<u32>>,
}
struct CounterWidget {
    count: u32,
    observed: Rc<Cell<u32>>,
}
impl View<()> for CounterView {
    type Element = CounterWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CounterWidget {
        CounterWidget {
            count: 0,
            observed: self.observed.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut CounterWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Refresh the observation handle but preserve the retained count.
        element.observed = self.observed.clone();
        ChangeFlags::NONE
    }
}
impl Widget for CounterWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        self.observed.set(self.count);
    }
    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
        {
            self.count += 1;
            ctx.request_redraw();
            return EventResult::Handled;
        }
        EventResult::Ignored
    }
}

pub(super) fn counter_page(observed: &Rc<Cell<u32>>) -> AnyView<()> {
    any(CounterView {
        observed: observed.clone(),
    })
}

/// A leaf that fills a rect of a fixed size — RecordingScene captures its
/// (origin, size) so a paint-culling test can tell pages apart by size.
pub(super) struct SizedLeaf {
    pub(super) size: Size,
}
impl<S: 'static> View<S> for SizedLeaf {
    type Element = SizedLeafWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SizedLeafWidget {
        SizedLeafWidget { size: self.size }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut SizedLeafWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}
pub(super) struct SizedLeafWidget {
    size: Size,
}
impl Widget for SizedLeafWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
    }
}
pub(super) fn sized_page<S: 'static>(w: f64, h: f64) -> AnyView<S> {
    any(SizedLeaf {
        size: Size::new(w, h),
    })
}

#[derive(Default)]
pub(super) struct ResultState {
    pub(super) received: Option<i32>,
}

/// The boxed app closure [`RenderRoot::rebuild`] drives, so a harness can
/// store one (mirroring `frust::back_glue`'s `AppLogic`).
pub(super) type AppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;

/// A recording scene that captures each fill's (origin, size) *and* the alpha
/// of the enclosing `push_layer` — so a transition test can assert both a
/// page's animated offset (origin) and its opacity.
#[derive(Default)]
pub(super) struct TransitionScene {
    pub(super) fills: Vec<(Point, Size, f32)>,
    layer_alpha: Vec<f32>,
}
impl PaintScene for TransitionScene {
    fn fill_rect(&mut self, origin: Point, size: Size, _color: peniko::Color) {
        let alpha = self.layer_alpha.last().copied().unwrap_or(1.0);
        self.fills.push((origin, size, alpha));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
        self.layer_alpha.push(alpha);
    }
    fn pop_layer(&mut self) {
        self.layer_alpha.pop();
    }
}

pub(super) fn ft(ms: u64) -> FrameTime {
    FrameTime::from_nanos(ms * 1_000_000)
}

/// Run one full frame (rebuild → layout → paint) at `time`, returning the
/// recorded fills and whether another frame was requested. Generic over
/// `State` so a result-carrying transition test can share it too.
pub(super) fn full_frame<State: 'static>(
    root: &mut RenderRoot<State, NavigatorView<State>>,
    app: &mut impl FnMut(&mut State) -> NavigatorView<State>,
    state: &mut State,
    time: FrameTime,
) -> (Vec<(Point, Size, f32)>, bool) {
    root.rebuild(app, state);
    root.layout(Size::new(100.0, 100.0));
    let mut scene = TransitionScene::default();
    let out = root.paint(&mut scene, time);
    (scene.fills, out.needs_frame)
}

/// Find the fill for the page of the given height (tests tag pages A/B by a
/// distinct height).
pub(super) fn fill_h(fills: &[(Point, Size, f32)], h: f64) -> (Point, Size, f32) {
    *fills
        .iter()
        .find(|(_, s, _)| (s.height - h).abs() < 1e-9)
        .unwrap_or_else(|| panic!("no fill with height {h} in {fills:?}"))
}

/// Downcast the root widget to a `&NavigatorWidget` so a gesture test can
/// inspect the private edge/transition state.
pub(super) fn nav_widget<S: 'static>(
    root: &RenderRoot<S, NavigatorView<S>>,
) -> &NavigatorWidget<S> {
    let id = root.root_id().expect("root built");
    (root.tree().pod(id).expect("root pod").widget() as &dyn Any)
        .downcast_ref::<NavigatorWidget<S>>()
        .expect("root is a NavigatorWidget")
}

/// Drive one shell-style frame (rebuild → layout → paint) at `time`, returning
/// the alpha-tagged fills and whether another frame was requested. Sharing the
/// `TransitionScene`/`full_frame`/`fill_h`/`ft` helpers above.
/// Run frames until the tree stops requesting them (a settle finishes and the
/// transition finalizes), returning the last frame's fills.
pub(super) fn run_until_settled(
    root: &mut RenderRoot<(), NavigatorView<()>>,
    app: &mut impl FnMut(&mut ()) -> NavigatorView<()>,
    state: &mut (),
    mut time_ms: u64,
) -> Vec<(Point, Size, f32)> {
    for _ in 0..10_000 {
        let (fills, needs_frame) = full_frame(root, app, state, ft(time_ms));
        if !needs_frame {
            return fills;
        }
        time_ms += 16;
    }
    panic!("transition failed to settle");
}

#[derive(Default)]
pub(super) struct SwipeResultState {
    pub(super) popped: bool,
}

/// A full-screen stand-in for one piece of the app root (its content, its
/// chrome) or for an overlay pushed over it. It fills its whole box, counts
/// the pointer `Down`s that actually reach it, and contributes exactly one
/// labelled accessibility node — so a test reads the input reach and the
/// accessibility reach of the same thing as two sets of labels (the shape
/// `tests/semantics_tree.rs`'s R23 parity helper uses).
struct HostProbe {
    label: &'static str,
    hits: Rc<Cell<u32>>,
}
struct HostProbeWidget {
    label: &'static str,
    hits: Rc<Cell<u32>>,
}
impl View<()> for HostProbe {
    type Element = HostProbeWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> HostProbeWidget {
        HostProbeWidget {
            label: self.label,
            hits: self.hits.clone(),
        }
    }
    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut HostProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.label = self.label;
        element.hits = self.hits.clone();
        ChangeFlags::NONE
    }
}
impl Widget for HostProbeWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.max()
    }
    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), peniko::Color::BLACK);
    }
    fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
        {
            self.hits.set(self.hits.get() + 1);
            return EventResult::Handled;
        }
        EventResult::Ignored
    }
    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(frust_core::accesskit::Role::Button, |node| {
            node.set_label(self.label)
        });
    }
}

pub(super) fn host_probe(label: &'static str, hits: &Rc<Cell<u32>>) -> AnyView<()> {
    any(HostProbe {
        label,
        hits: hits.clone(),
    })
}

/// Every label present anywhere in the collected accessibility tree.
pub(super) fn semantic_labels(root: &RenderRoot<(), NavigatorView<()>>) -> Vec<String> {
    root.semantics()
        .nodes
        .iter()
        .filter_map(|(_, node)| node.label().map(str::to_string))
        .collect()
}

/// The app-root shape a host wraps: `Stack[content, chrome]`, both
/// full-screen, so the chrome paints over the content and — by
/// `StackWidget`'s reverse-order hit-testing — takes any pointer that
/// reaches the app root at all.
type HostAppLogic = Box<dyn FnMut(&mut ()) -> NavigatorView<()>>;
type HostFixture = (
    NavigatorController<()>,
    RenderRoot<(), NavigatorView<()>>,
    HostAppLogic,
    Rc<Cell<u32>>,
    Rc<Cell<u32>>,
);

pub(super) fn overlay_host_fixture() -> HostFixture {
    let controller: NavigatorController<()> = NavigatorController::new();
    let content_hits = Rc::new(Cell::new(0u32));
    let chrome_hits = Rc::new(Cell::new(0u32));
    let app: HostAppLogic = {
        let ctrl = controller.clone();
        let content = content_hits.clone();
        let chrome = chrome_hits.clone();
        Box::new(move |_: &mut ()| {
            let content = content.clone();
            let chrome = chrome.clone();
            overlay_host(&ctrl, move || {
                any(Stack(vec![
                    host_probe("app-content", &content),
                    host_probe("app-chrome", &chrome),
                ]))
            })
        })
    };
    (
        controller,
        RenderRoot::new(),
        app,
        content_hits,
        chrome_hits,
    )
}

pub(super) fn route(path: &str) -> Location {
    Location::parse(path)
}
