//! The layout skip, audited across the baseline widget set.
//!
//! Every scenario mounts a widget in a [`RenderRoot`] with
//! `set_layout_audit(true)` and drives it through its scripted interactions —
//! drag, fling, `jump_to`, `animate_to`, wheel, an edge-swipe pop, a tab
//! switch, an IME commit, a drop — for at least [`MIN_FRAMES`] frames. The
//! audit runs a forced full layout after every skipping one and counts each pod
//! whose geometry differs; a widget that changes layout-affecting state without
//! signalling dirty (see `Widget::layout`'s *Layout may be skipped*) shows up
//! here as a non-zero `layout_audit_mismatches()`.
//!
//! The audit itself only counts (its named report goes to the `log` crate,
//! which this crate does not depend on), so a failing scenario is re-run in a
//! *manual* mode that performs the same comparison by hand — a skipping pass,
//! a snapshot, a forced pass, a snapshot — and the panic names every widget
//! type whose geometry differed and the script step it differed on.
//!
//! Every scenario then runs a second time **gated**, the way the shells drive
//! a root: a frame lays out only when `take_change_flags().needs_layout()`
//! asks. The audit cannot see a dirtying channel that never reaches the
//! pending flags — it runs inside a pass, and the gate skips the pass — so a
//! frame the gate skips is checked instead: a forced pass must find every
//! pod's geometry already current. A difference there is a pod the shells
//! would have left stale.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, Curve, DiscardScene, FrameTime, ImeEvent,
    InputEvent, Key, KeyEvent, LayoutCtx, Modifiers, NamedKey, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, RenderRoot, ScrollDelta, View, Widget, WidgetId, WindowEdgeInsets,
    WindowInsets, any,
};
use frust_text::TextContext;
use frust_widgets::motion::patterns::FadeThrough;
use frust_widgets::motion::switcher::pattern_switcher;
use frust_widgets::motion::{animated_opacity, animated_scale};
use frust_widgets::scroll_controller::ItemAlignment;
use frust_widgets::{
    AnimateTo, ChildKey, Column, DragCoordinator, DragPolicy, GestureDetector, ListView,
    NavigatorController, OverlayPlacement, OverlaySide, PageTransition, PanZoomController,
    ScrollController, SizedBox, Stack, TransitionSpec, auto_scroll_zone, button, canvas, checkbox,
    drag_target, draggable, indexed_stack, list_view, navigator, overlay_portal, pan_zoom,
    platform_view, reorderable_list, safe_area, scaffold, scroll_view, slider, text, text_input,
};
use kurbo::{Point, Rect, Size, Vec2};

/// Every scenario runs at least this many frames.
const MIN_FRAMES: u32 = 60;
const WINDOW: Size = Size::new(400.0, 600.0);
const FRAME_MS: f64 = 16.0;

// ---------------------------------------------------------------------------
// Harness.
// ---------------------------------------------------------------------------

/// How a harness checks each layout pass.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `RenderRoot::set_layout_audit(true)` — the acceptance check.
    Audit,
    /// The same comparison by hand, so a mismatch can be named.
    Manual,
    /// A plain skipping pass, nothing checked — for counting `layout` calls,
    /// which the other two modes' forced passes would inflate.
    Plain,
}

type Logic<S> = Box<dyn FnMut(&mut S) -> AnyView<S>>;

struct Harness<S: 'static> {
    root: RenderRoot<S, AnyView<S>>,
    state: S,
    logic: Logic<S>,
    tcx: TextContext,
    mode: Mode,
    ms: f64,
    frames: u32,
    step: &'static str,
    /// Manual mode: `step: Type` for every pod whose geometry differed (and,
    /// gated, every pod a skipped frame left stale, in any checking mode).
    named: Vec<String>,
    /// Lay out only when the pending flags ask (see [`Harness::frame_gated`]).
    gated: bool,
    /// Gated: pods a frame the gate skipped left with stale geometry.
    gate_misses: u64,
}

impl<S: 'static> Harness<S> {
    /// Mount `logic`'s view under a window-sized box: the root widget itself
    /// sits in no `ChildPod` and is laid out every pass, so the widget under
    /// test must be one level down for the skip to reach it at all.
    fn new(mode: Mode, state: S, mut logic: impl FnMut(&mut S) -> AnyView<S> + 'static) -> Self {
        let mut root = RenderRoot::new();
        root.set_layout_audit(mode == Mode::Audit);
        let mut harness = Self {
            root,
            state,
            logic: Box::new(move |state: &mut S| {
                any(SizedBox::<S>(Some(WINDOW.width), Some(WINDOW.height)).child(logic(state)))
            }),
            tcx: TextContext::new(),
            mode,
            ms: 0.0,
            frames: 0,
            step: "mount",
            named: Vec::new(),
            gated: false,
            gate_misses: 0,
        };
        harness.frames(3);
        harness
    }

    /// Switch to gated frames. The mount frames above ran ungated, standing in
    /// for the shells' forced first frame; whatever they left pending is
    /// drained by the first gated frame.
    fn gate(mut self) -> Self {
        self.gated = true;
        self
    }

    fn step(&mut self, step: &'static str) {
        self.step = step;
    }

    /// Every pod's `(type, origin relative to parent, size)`, keyed by id.
    fn geometry(&self) -> HashMap<WidgetId, (&'static str, Point, Size)> {
        let nodes = self.root.inspect();
        let origins: HashMap<WidgetId, Point> =
            nodes.iter().map(|n| (n.id, n.bounds.origin())).collect();
        nodes
            .iter()
            .map(|n| {
                let parent = n
                    .parent
                    .and_then(|p| origins.get(&p).copied())
                    .unwrap_or(Point::ZERO);
                let origin = Point::new(n.bounds.x0 - parent.x, n.bounds.y0 - parent.y);
                (n.id, (n.type_name, origin, n.bounds.size()))
            })
            .collect()
    }

    fn layout(&mut self) {
        match self.mode {
            Mode::Audit | Mode::Plain => {
                self.root.layout_with_text(WINDOW, &mut self.tcx);
            }
            Mode::Manual => {
                self.root.layout_with_text(WINDOW, &mut self.tcx);
                self.forced_pass_differs("");
            }
        }
    }

    /// Run a forced pass over the current geometry and record (as
    /// `step: Type`, with `tag` before the type) every pod it moved or
    /// resized. Returns how many differed.
    fn forced_pass_differs(&mut self, tag: &str) -> u64 {
        let before = self.geometry();
        self.root.set_layout_skip(false);
        self.root.layout_with_text(WINDOW, &mut self.tcx);
        self.root.set_layout_skip(true);
        let mut count = 0;
        for (id, (name, origin, size)) in self.geometry() {
            let Some((_, was_origin, was_size)) = before.get(&id) else {
                continue;
            };
            let differs = (was_origin.x - origin.x).abs() > 1e-3
                || (was_origin.y - origin.y).abs() > 1e-3
                || (was_size.width - size.width).abs() > 1e-3
                || (was_size.height - size.height).abs() > 1e-3;
            if differs {
                count += 1;
                let short = name.rsplit("::").next().unwrap_or(name);
                let entry = format!("{}: {tag}{short}", self.step);
                if !self.named.contains(&entry) {
                    self.named.push(entry);
                }
            }
        }
        count
    }

    /// One whole frame: rebuild, layout (checked), paint.
    fn frame(&mut self) {
        if self.gated {
            self.frame_gated();
            return;
        }
        self.root.rebuild(&mut self.logic, &mut self.state);
        self.layout();
        self.paint();
    }

    /// One whole frame the way the shells run it: rebuild, then lay out only
    /// when `take_change_flags().needs_layout()` asks (checked like any other
    /// pass), then paint. A frame the gate skips is checked too, outside the
    /// plain mode: a forced pass must find nothing to move — anything it does
    /// move is a pod some channel dirtied without reaching the pending flags.
    fn frame_gated(&mut self) {
        self.root.rebuild(&mut self.logic, &mut self.state);
        if self.root.take_change_flags().needs_layout() {
            self.layout();
        } else if self.mode != Mode::Plain {
            self.gate_misses += self.forced_pass_differs("gate skipped ");
        }
        self.paint();
    }

    fn paint(&mut self) {
        self.ms += FRAME_MS;
        self.root.paint(
            &mut DiscardScene,
            FrameTime::from_nanos((self.ms * 1_000_000.0) as u64),
        );
        self.frames += 1;
    }

    fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame();
        }
    }

    fn pointer(&mut self, phase: PointerPhase, at: Point) {
        self.root.event(
            &mut self.state,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: at,
                button: PointerButton::Primary,
            }),
        );
    }

    /// Press at `from`, move to `to` in `steps` moves with a frame after each,
    /// and release.
    fn drag(&mut self, from: Point, to: Point, steps: u32) {
        self.pointer(PointerPhase::Down, from);
        self.frame();
        for i in 1..=steps {
            let t = f64::from(i) / f64::from(steps);
            self.pointer(PointerPhase::Move, from.lerp(to, t));
            self.frame();
        }
        self.pointer(PointerPhase::Up, to);
        self.frame();
    }

    fn tap(&mut self, at: Point) {
        self.pointer(PointerPhase::Down, at);
        self.frame();
        self.pointer(PointerPhase::Up, at);
        self.frame();
    }

    fn wheel(&mut self, at: Point, dy: f64) {
        // A hover move first, the way a desktop shell delivers one ahead of
        // the wheel.
        self.pointer(PointerPhase::Move, at);
        self.root.event(
            &mut self.state,
            &InputEvent::Scroll {
                delta: ScrollDelta::Pixels(0.0, dy),
                position: at,
            },
        );
        self.frame();
    }

    fn event(&mut self, event: InputEvent) {
        self.root.event(&mut self.state, &event);
    }

    /// The window-space center of the first laid-out widget whose type name
    /// contains `type_name`.
    fn center_of(&self, type_name: &str) -> Point {
        self.root
            .inspect()
            .iter()
            .find(|n| n.type_name.contains(type_name))
            .map(|n| n.bounds.center())
            .unwrap_or_else(|| panic!("no {type_name} in the tree"))
    }
}

/// Run `script` under the audit, then again gated (see [`Harness::frame_gated`]);
/// on any mismatch re-run it in manual mode (gated alike) and panic naming
/// every widget whose geometry the skip — or the gate — left stale.
fn check<S: 'static>(
    name: &str,
    make: impl Fn(Mode) -> Harness<S>,
    script: impl Fn(&mut Harness<S>),
) {
    for gated in [false, true] {
        let mut audited = make(Mode::Audit);
        if gated {
            audited = audited.gate();
        }
        script(&mut audited);
        assert!(
            audited.frames >= MIN_FRAMES,
            "{name}: the script ran {} frames, fewer than {MIN_FRAMES}",
            audited.frames
        );
        let mismatches = audited.root.layout_audit_mismatches() + audited.gate_misses;
        if mismatches != 0 {
            let mut manual = make(Mode::Manual);
            if gated {
                manual = manual.gate();
            }
            script(&mut manual);
            panic!(
                "{name} (gated: {gated}): the layout audit found {mismatches} stale pod(s); by hand: {:#?}",
                manual.named
            );
        }
    }
}

fn rows(n: usize, height: f64) -> Vec<AnyView<()>> {
    (0..n)
        .map(|_| any(SizedBox::<()>(Some(WINDOW.width), Some(height))))
        .collect()
}

fn tween(ms: f64) -> AnimateTo {
    AnimateTo {
        duration_ms: ms,
        curve: Curve::EaseInOut,
    }
}

const MID: Point = Point::new(200.0, 300.0);

// ---------------------------------------------------------------------------
// Scroll surfaces: ScrollView and ListView, scripted identically (parity).
// ---------------------------------------------------------------------------

/// Drag, fling, overscroll + settle, wheel, `jump_to` (also mid-fling and
/// back-to-back), `animate_to` and an animate replaced by a jump.
fn scroll_script<S: 'static>(h: &mut Harness<S>, controller: &ScrollController) {
    h.step("drag + fling");
    h.drag(Point::new(200.0, 500.0), Point::new(200.0, 150.0), 8);
    h.frames(30);
    assert!(controller.offset() > 300.0, "the drag scrolled the surface");
    h.step("overscroll pull + settle");
    controller.jump_to(0.0);
    h.frame();
    h.drag(Point::new(200.0, 100.0), Point::new(200.0, 400.0), 6);
    h.frames(20);
    h.step("wheel");
    h.wheel(MID, 120.0);
    h.wheel(MID, -40.0);
    h.step("jump_to");
    controller.jump_to(900.0);
    h.frames(3);
    controller.jump_to(450.0);
    controller.jump_to(460.0);
    h.frames(2);
    h.step("jump_to mid-fling");
    h.drag(Point::new(200.0, 500.0), Point::new(200.0, 100.0), 4);
    h.frames(3);
    controller.jump_to(200.0);
    h.frames(5);
    h.step("animate_to");
    controller.animate_to(1200.0, tween(250.0));
    h.frames(25);
    h.step("animate_to replaced by jump_to");
    controller.animate_to(0.0, tween(400.0));
    h.frames(5);
    controller.jump_to(700.0);
    h.frames(5);
    h.step("jump_to past the end");
    controller.jump_to(f64::INFINITY);
    h.frames(3);
    assert!(
        controller.offset() > 0.0 && controller.offset() == controller.max_offset(),
        "the last jump landed on the bottom edge"
    );
}

#[test]
fn scroll_view_drag_fling_jump_and_animate_stay_in_sync() {
    let controller = ScrollController::new();
    let make = {
        let controller = controller.clone();
        move |mode| {
            let controller = controller.clone();
            Harness::new(mode, (), move |_: &mut ()| {
                any(scroll_view(Column(rows(60, 50.0))).controller(controller.clone()))
            })
        }
    };
    // The audited run and a manual re-run share the handle: the re-run's
    // surface attaches it last and takes it over (the most recent attach wins).
    check("scroll_view", make, |h| scroll_script(h, &controller));
}

#[test]
fn list_view_drag_fling_jump_and_animate_stay_in_sync() {
    let controller = ScrollController::new();
    let make = {
        let controller = controller.clone();
        move |mode| {
            let controller = controller.clone();
            Harness::new(mode, (), move |_: &mut ()| {
                any(list_view(400, 40.0, |_| SizedBox::<()>(None, Some(40.0)))
                    .controller(controller.clone()))
            })
        }
    };
    check("list_view", make, |h| scroll_script(h, &controller));
}

/// A scroll surface whose content changes size under it, the jump recorded in
/// the same frame as the growth (lands against the new extent).
#[test]
fn scroll_view_content_growth_with_a_jump_stays_in_sync() {
    let controller = ScrollController::new();
    let count = Rc::new(Cell::new(20usize));
    let make = {
        let (controller, count) = (controller.clone(), count.clone());
        move |mode| {
            let (controller, count) = (controller.clone(), count.clone());
            count.set(20);
            Harness::new(mode, (), move |_: &mut ()| {
                any(scroll_view(Column(rows(count.get(), 50.0))).controller(controller.clone()))
            })
        }
    };
    check("scroll_view growth", make, |h| {
        for round in 0..30 {
            h.step("grow + jump to the new bottom");
            count.set(20 + round * 3);
            controller.jump_to(f64::INFINITY);
            h.frames(2);
        }
    });
}

// ---------------------------------------------------------------------------
// PanZoom.
// ---------------------------------------------------------------------------

#[test]
fn pan_zoom_pan_glide_and_controller_stay_in_sync() {
    let controller = PanZoomController::new();
    let make = {
        let controller = controller.clone();
        move |mode| {
            let controller = controller.clone();
            Harness::new(mode, (), move |_: &mut ()| {
                any(pan_zoom(SizedBox::<()>(Some(1000.0), Some(800.0)))
                    .controller(controller.clone()))
            })
        }
    };
    check("pan_zoom", make, |h| {
        h.step("pan + glide");
        h.drag(Point::new(300.0, 300.0), Point::new(100.0, 150.0), 6);
        h.frames(30);
        assert_ne!(controller.transform().offset, Vec2::ZERO, "the drag panned");
        h.step("wheel");
        h.wheel(MID, 60.0);
        h.step("controller jump_to");
        controller.jump_to(2.0, Vec2::new(-150.0, -80.0));
        h.frames(5);
        h.step("controller fit_to_bounds");
        controller.fit_to_bounds();
        h.frames(5);
        h.step("controller fit_rect");
        controller.fit_rect(Rect::new(100.0, 100.0, 300.0, 300.0));
        h.frames(10);
    });
}

// ---------------------------------------------------------------------------
// Navigator: programmatic push/pop transitions and the interactive edge swipe.
// ---------------------------------------------------------------------------

fn page(label: &'static str) -> impl View<()> {
    scroll_view(Column(
        std::iter::once(any(text(label)))
            .chain(rows(30, 40.0))
            .collect::<Vec<_>>(),
    ))
}

#[test]
fn navigator_transitions_and_edge_swipe_stay_in_sync() {
    let nav: NavigatorController<()> = NavigatorController::new();
    let make = {
        let nav = nav.clone();
        move |mode| {
            let nav = nav.clone();
            Harness::new(mode, (), move |_: &mut ()| {
                any(navigator(&nav, || page("home"))
                    .transition(TransitionSpec::duration(PageTransition::IosPush)))
            })
        }
    };
    check("navigator", make, |h| {
        h.step("push");
        nav.push(|| page("second"));
        h.frames(30);
        assert_eq!(nav.depth(), 2);
        h.step("pop");
        nav.pop();
        h.frames(30);
        h.step("push transparent");
        nav.push_transparent(|| text("dialog"));
        h.frames(30);
        h.step("edge-swipe pop");
        assert_eq!(nav.depth(), 2);
        h.drag(Point::new(4.0, 300.0), Point::new(320.0, 310.0), 10);
        h.frames(30);
        assert_eq!(nav.depth(), 1, "the edge swipe popped the page");
        h.step("cancelled edge swipe");
        nav.push(|| page("third"));
        h.frames(30);
        h.drag(Point::new(4.0, 300.0), Point::new(40.0, 300.0), 4);
        h.frames(30);
        assert_eq!(nav.depth(), 2, "the short swipe cancelled");
        h.step("pop after cancelled swipe");
        nav.pop();
        h.frames(30);
    });
}

// ---------------------------------------------------------------------------
// IndexedStack: tab switches, hidden tabs' scroll surfaces driven while hidden.
// ---------------------------------------------------------------------------

#[test]
fn indexed_stack_tab_switches_stay_in_sync() {
    let tab = Rc::new(Cell::new(0usize));
    let first = ScrollController::new();
    let second = ScrollController::new();
    let make = {
        let (tab, first, second) = (tab.clone(), first.clone(), second.clone());
        move |mode| {
            let (tab, first, second) = (tab.clone(), first.clone(), second.clone());
            tab.set(0);
            Harness::new(mode, (), move |_: &mut ()| {
                any(indexed_stack(Some(tab.get()))
                    .child(scroll_view(Column(rows(40, 50.0))).controller(first.clone()))
                    .child(
                        list_view(200, 40.0, |_| SizedBox::<()>(None, Some(40.0)))
                            .controller(second.clone()),
                    )
                    .child(Column(rows(3, 80.0))))
            })
        }
    };
    check("indexed_stack", make, |h| {
        h.step("scroll the first tab");
        h.drag(Point::new(200.0, 500.0), Point::new(200.0, 200.0), 5);
        h.frames(10);
        assert!(first.offset() > 0.0, "the drag scrolled the first tab");
        h.step("switch to the list tab");
        tab.set(1);
        h.frames(5);
        h.step("jump the hidden first tab");
        first.jump_to(600.0);
        h.frames(5);
        h.step("fling the list tab");
        h.drag(Point::new(200.0, 500.0), Point::new(200.0, 100.0), 4);
        h.frames(20);
        h.step("back to the first tab");
        tab.set(0);
        h.frames(5);
        h.step("jump the hidden list tab, then reveal it");
        second.jump_to(2000.0);
        h.frame();
        tab.set(1);
        h.frames(5);
        h.step("static tab");
        tab.set(2);
        h.frames(10);
    });
}

// ---------------------------------------------------------------------------
// Switcher and the implicit animations: paint-only after the rebuild.
// ---------------------------------------------------------------------------

#[test]
fn switcher_and_animated_wrappers_stay_in_sync() {
    let key = Rc::new(Cell::new(0u32));
    let make = {
        let key = key.clone();
        move |mode| {
            let key = key.clone();
            key.set(0);
            Harness::new(mode, (), move |_: &mut ()| {
                let k = key.get();
                let height = 40.0 + f64::from(k % 3) * 30.0;
                any(Column((
                    pattern_switcher(k, FadeThrough, SizedBox::<()>(Some(200.0), Some(height))),
                    animated_opacity(
                        if k.is_multiple_of(2) { 1.0 } else { 0.2 },
                        SizedBox::<()>(Some(100.0), Some(50.0)),
                    ),
                    animated_scale(
                        if k.is_multiple_of(2) { 1.0 } else { 1.5 },
                        SizedBox::<()>(Some(100.0), Some(50.0)),
                    ),
                    text(format!("key {k}")),
                )))
            })
        }
    };
    check("switcher/animated", make, |h| {
        for k in 1..=6 {
            h.step("switch key");
            key.set(k);
            h.frames(12);
        }
    });
}

// ---------------------------------------------------------------------------
// Drag and drop: draggable/drag_target, edge auto-scroll, reorderable list.
// ---------------------------------------------------------------------------

#[test]
fn drag_auto_scroll_and_reorder_stay_in_sync() {
    let coordinator = DragCoordinator::new();
    let controller = ScrollController::new();
    let reordered = Rc::new(Cell::new(None::<(usize, usize)>));
    let make = {
        let (coordinator, controller, reordered) =
            (coordinator.clone(), controller.clone(), reordered.clone());
        move |mode| {
            let (coordinator, controller, reordered) =
                (coordinator.clone(), controller.clone(), reordered.clone());
            Harness::new(mode, (), move |_: &mut ()| {
                let mut content: Vec<AnyView<()>> = vec![any(draggable(
                    SizedBox::<()>(Some(120.0), Some(40.0)),
                    coordinator.clone(),
                    |_: &()| 7u32,
                )
                .policy(DragPolicy::Immediate))];
                for _ in 0..20 {
                    content.push(any(drag_target::<u32, (), _>(
                        SizedBox::<()>(Some(WINDOW.width), Some(60.0)),
                        coordinator.clone(),
                    )));
                }
                let surface = scroll_view(Column(content)).controller(controller.clone());
                let zone = auto_scroll_zone(surface, coordinator.clone(), controller.clone());
                let reorder = reorderable_list(
                    coordinator.clone(),
                    (0..4u32).map(|i| (i, SizedBox::<()>(Some(WINDOW.width), Some(30.0)))),
                )
                .on_reorder({
                    let reordered = reordered.clone();
                    move |_: &mut (), from, to| reordered.set(Some((from, to)))
                });
                any(Column((
                    SizedBox::<()>(None, Some(300.0)).child(zone),
                    reorder,
                )))
            })
        }
    };
    check("drag", make, |h| {
        h.step("drag to the bottom edge (auto-scroll)");
        h.pointer(PointerPhase::Down, Point::new(20.0, 20.0));
        h.frame();
        // Sideways first, so the press becomes a drag before the scroll
        // surface could read it as a vertical scroll.
        h.pointer(PointerPhase::Move, Point::new(60.0, 20.0));
        h.frame();
        for i in 1..=6 {
            h.pointer(
                PointerPhase::Move,
                Point::new(60.0, 20.0 + f64::from(i) * 45.0),
            );
            h.frame();
        }
        h.frames(25);
        assert!(controller.offset() > 0.0, "the edge band auto-scrolled");
        h.step("drop");
        h.pointer(PointerPhase::Up, Point::new(20.0, 290.0));
        h.frames(5);
        h.step("reorder: drag row 0 below row 2");
        // The list sits under the 300 px zone: gap 8, row 30, gap 8, …, so
        // row 0 spans 308..338 and the gap after row 2 spans 414..422.
        reordered.set(None);
        h.drag(Point::new(50.0, 320.0), Point::new(50.0, 418.0), 8);
        h.frames(10);
        assert_eq!(reordered.get(), Some((0, 2)), "the drop reordered the list");
    });
}

// ---------------------------------------------------------------------------
// TextInput: focus, IME commits growing a multiline field, edits.
// ---------------------------------------------------------------------------

#[test]
fn text_input_ime_commits_stay_in_sync() {
    #[derive(Default)]
    struct App {
        value: String,
    }
    let make = |mode| {
        Harness::new(mode, App::default(), |state: &mut App| {
            any(Column((
                text_input(state.value.clone(), |state: &mut App, value| {
                    state.value = value
                })
                .multiline(5),
                text(format!("{} chars", state.value.len())),
            )))
        })
    };
    check("text_input", make, |h| {
        h.step("focus");
        let at = h.center_of("TextInputWidget");
        h.tap(at);
        for line in 0..6 {
            h.step("IME commit");
            h.event(InputEvent::Ime(ImeEvent::Commit(format!("line {line}"))));
            h.frame();
            h.event(InputEvent::Ime(ImeEvent::Commit("\n".into())));
            h.frames(3);
        }
        h.step("compose");
        h.event(InputEvent::Ime(ImeEvent::Compose {
            text: "compos".into(),
            cursor: Some((6, 6)),
        }));
        h.frames(5);
        h.event(InputEvent::Ime(ImeEvent::Commit("composed".into())));
        h.frames(30);
        assert!(
            h.state.value.contains("line 5") && h.state.value.contains("composed"),
            "the commits reached the field: {:?}",
            h.state.value
        );
    });
}

/// The lone-field shape: a multiline field with no sibling whose rebuild would
/// report `LAYOUT` for it. Its growth reaches the pending flags only through
/// the event itself (the edit is consumed and asks for a redraw) — the
/// controlled value the next rebuild hands back already equals the editor's
/// text, so that rebuild reports nothing.
#[test]
fn a_lone_multiline_text_input_grows_under_the_gate() {
    #[derive(Default)]
    struct App {
        value: String,
    }
    let mut h = Harness::new(Mode::Plain, App::default(), |state: &mut App| {
        any(Column(
            text_input(state.value.clone(), |state: &mut App, value| {
                state.value = value
            })
            .multiline(5),
        ))
    })
    .gate();
    let key = |key: Key| {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    };
    let height = |h: &Harness<App>| {
        h.root
            .inspect()
            .iter()
            .find(|n| n.type_name.contains("TextInputWidget"))
            .map(|n| n.bounds.height())
            .expect("the field is in the tree")
    };
    h.step("focus");
    let at = h.center_of("TextInputWidget");
    h.tap(at);
    h.frames(2);
    let one_line = height(&h);
    h.step("Enter + text");
    h.event(key(Key::Character("a".into())));
    h.frame();
    h.event(key(Key::Named(NamedKey::Enter)));
    h.frame();
    h.event(key(Key::Character("b".into())));
    h.frames(2);
    assert_eq!(h.state.value, "a\nb", "the edits reached on_change");
    assert!(
        height(&h) > one_line,
        "the field grew to two lines under the gate: {one_line} -> {}",
        height(&h)
    );
}

// ---------------------------------------------------------------------------
// Chrome and leaves: scaffold, safe_area, stack, button, gesture, overlay
// portal, canvas, platform_view — under inset changes and presses.
// ---------------------------------------------------------------------------

#[test]
fn chrome_and_leaf_widgets_stay_in_sync() {
    let open = Rc::new(Cell::new(false));
    let taps = Rc::new(Cell::new(0u32));
    let make = {
        let (open, taps) = (open.clone(), taps.clone());
        move |mode| {
            let (open, taps) = (open.clone(), taps.clone());
            open.set(false);
            Harness::new(mode, (), move |_: &mut ()| {
                let taps_for_tap = taps.clone();
                let body = safe_area(Column((
                    button("press", |_: &mut ()| {}),
                    slider(0.5, |_: &mut (), _| {}),
                    GestureDetector(SizedBox::<()>(Some(200.0), Some(60.0)))
                        .on_tap(move |_: &mut ()| taps_for_tap.set(taps_for_tap.get() + 1))
                        .on_long_press(|_: &mut ()| {}),
                    overlay_portal(SizedBox::<()>(Some(80.0), Some(30.0)))
                        .overlay(open.get().then(|| text("floating")))
                        .placement(OverlayPlacement::on(OverlaySide::Bottom)),
                    SizedBox::<()>(Some(200.0), Some(80.0)).child(canvas(|_, _, _| {})),
                    platform_view("probe").size(120.0, 60.0),
                    Stack((
                        SizedBox::<()>(Some(100.0), Some(40.0)),
                        text(format!("taps {}", taps.get())),
                    )),
                )));
                any(scaffold(body).app_bar(text("bar")))
            })
        }
    };
    check("chrome", make, |h| {
        h.step("button press");
        let at = h.center_of("ButtonWidget");
        h.tap(at);
        h.frames(5);
        h.step("slider drag");
        let at = h.center_of("SliderWidget");
        h.drag(at, at + Vec2::new(120.0, 0.0), 6);
        h.frames(5);
        h.step("gesture tap");
        let at = h.center_of("GestureDetectorWidget");
        h.tap(at);
        h.frames(5);
        h.step("gesture long press");
        h.pointer(PointerPhase::Down, at);
        h.frames(40);
        h.pointer(PointerPhase::Up, at);
        h.frames(3);
        h.step("open overlay");
        open.set(true);
        h.frames(5);
        h.step("insets change");
        let edges = WindowEdgeInsets {
            left: 0.0,
            top: 24.0,
            right: 0.0,
            bottom: 16.0,
        };
        h.root
            .set_insets(WindowInsets::new(edges, WindowEdgeInsets::ZERO));
        h.frames(5);
        h.step("keyboard inset");
        h.root.set_insets(WindowInsets::new(
            edges,
            WindowEdgeInsets {
                bottom: 280.0,
                ..WindowEdgeInsets::ZERO
            },
        ));
        h.frames(5);
        h.step("close overlay");
        open.set(false);
        h.frames(5);
    });
}

// ---------------------------------------------------------------------------
// ListView: keyed, variable extents — `scroll_to_item`, measured corrections,
// prepend anchoring.
// ---------------------------------------------------------------------------

#[test]
fn keyed_variable_list_scroll_to_item_and_prepend_stay_in_sync() {
    let controller = ScrollController::new();
    // Item ids are `base..base + count`; a prepend lowers `base`.
    let base = Rc::new(Cell::new(1000u64));
    let count = Rc::new(Cell::new(300usize));
    let make = {
        let (controller, base, count) = (controller.clone(), base.clone(), count.clone());
        move |mode| {
            let (controller, base, count) = (controller.clone(), base.clone(), count.clone());
            base.set(1000);
            count.set(300);
            Harness::new(mode, (), move |_: &mut ()| {
                let (b1, b2) = (base.get(), base.get());
                any(ListView::builder_keyed(
                    count.get(),
                    40.0,
                    move |i| ChildKey::new(b1 + i as u64),
                    move |i| {
                        let id = b2 + i as u64;
                        SizedBox::<()>(None, Some(24.0 + (id % 5) as f64 * 14.0))
                    },
                )
                .estimated_item_extent(40.0)
                .controller(controller.clone()))
            })
        }
    };
    check("keyed variable list_view", make, |h| {
        h.step("scroll_to_item jump");
        controller.scroll_to_item(ChildKey::new(1150u64), ItemAlignment::Center, false);
        h.frames(10);
        h.step("scroll_to_item animated");
        controller.scroll_to_item(ChildKey::new(1040u64), ItemAlignment::Start, true);
        h.frames(30);
        h.step("fling");
        h.drag(Point::new(200.0, 500.0), Point::new(200.0, 100.0), 5);
        h.frames(20);
        h.step("prepend");
        base.set(base.get() - 10);
        count.set(count.get() + 10);
        h.frames(5);
        h.step("scroll_to_item end");
        controller.scroll_to_item(ChildKey::new(1299u64), ItemAlignment::End, false);
        h.frames(10);
    });
}

// ---------------------------------------------------------------------------
// Nested scroll: a list inside a scroll view, dragged past its own end so the
// outer surface takes the gesture over.
// ---------------------------------------------------------------------------

#[test]
fn nested_scroll_surfaces_stay_in_sync() {
    let inner = ScrollController::new();
    let make = {
        let inner = inner.clone();
        move |mode| {
            let inner = inner.clone();
            Harness::new(mode, (), move |_: &mut ()| {
                let mut content: Vec<AnyView<()>> = vec![any(SizedBox::<()>(None, Some(80.0)))];
                content.push(any(SizedBox::<()>(Some(WINDOW.width), Some(300.0)).child(
                    list_view(40, 40.0, |_| SizedBox::<()>(None, Some(40.0)))
                        .controller(inner.clone()),
                )));
                content.extend(rows(20, 50.0));
                any(scroll_view(Column(content)))
            })
        }
    };
    check("nested scroll", make, |h| {
        h.step("inner drag + fling");
        h.drag(Point::new(200.0, 350.0), Point::new(200.0, 120.0), 6);
        h.frames(25);
        h.step("inner jump to its end, then drag on into the outer");
        inner.jump_to(f64::INFINITY);
        h.frames(3);
        h.drag(Point::new(200.0, 350.0), Point::new(200.0, 90.0), 8);
        h.frames(25);
        h.step("outer drag back");
        h.drag(Point::new(200.0, 100.0), Point::new(200.0, 500.0), 6);
        h.frames(25);
    });
}

// ---------------------------------------------------------------------------
// Single-line TextInput: typing past the field width, caret navigation.
// ---------------------------------------------------------------------------

#[test]
fn single_line_text_input_typing_stays_in_sync() {
    #[derive(Default)]
    struct App {
        value: String,
    }
    let make = |mode| {
        Harness::new(mode, App::default(), |state: &mut App| {
            any(Column((
                SizedBox::<App>(Some(160.0), None)
                    .child(text_input(state.value.clone(), |state: &mut App, value| {
                        state.value = value
                    })),
                checkbox(
                    state.value.len().is_multiple_of(2),
                    "even",
                    |_: &mut App, _| {},
                ),
            )))
        })
    };
    let key = |key: Key| {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    };
    check("single-line text_input", make, |h| {
        h.step("focus");
        let at = h.center_of("TextInputWidget");
        h.tap(at);
        h.step("type past the width");
        for c in "the quick brown fox jumps over the lazy dog".chars() {
            h.event(key(Key::Character(c.to_string())));
            h.frame();
        }
        h.step("caret home + backspace");
        h.event(key(Key::Named(NamedKey::Home)));
        h.frames(3);
        for _ in 0..5 {
            h.event(key(Key::Named(NamedKey::ArrowRight)));
            h.frame();
        }
        h.event(key(Key::Named(NamedKey::Backspace)));
        h.frames(10);
        assert_eq!(h.state.value, "the uick brown fox jumps over the lazy dog");
    });
}

// ---------------------------------------------------------------------------
// The skip actually skips: a hidden IndexedStack tab is not laid out while
// another tab changes. The stack still lays out every child every pass (one
// size across selection changes), but under the per-pod skip a hidden,
// unchanged tab costs one clean-pod check.
// ---------------------------------------------------------------------------

thread_local! {
    static LAYOUTS: RefCell<HashMap<&'static str, u32>> = RefCell::new(HashMap::new());
}

fn layouts_of(tag: &'static str) -> u32 {
    LAYOUTS.with(|l| l.borrow().get(tag).copied().unwrap_or(0))
}

/// A fixed-size leaf counting its own `layout` calls under `tag`.
struct Counted {
    tag: &'static str,
    height: f64,
}

struct CountedWidget {
    tag: &'static str,
    height: f64,
}

impl View<()> for Counted {
    type Element = CountedWidget;
    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CountedWidget {
        CountedWidget {
            tag: self.tag,
            height: self.height,
        }
    }
    fn rebuild(&self, prev: &Self, el: &mut CountedWidget, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        if self.height == prev.height {
            return ChangeFlags::NONE;
        }
        el.height = self.height;
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

impl Widget for CountedWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        LAYOUTS.with(|l| *l.borrow_mut().entry(self.tag).or_insert(0) += 1);
        bc.constrain(Size::new(100.0, self.height))
    }
    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
}

#[test]
fn a_hidden_indexed_stack_tab_is_skipped_while_the_shown_tab_changes() {
    let tab = Rc::new(Cell::new(0usize));
    let height = Rc::new(Cell::new(20.0f64));
    let mut h = {
        let (tab, height) = (tab.clone(), height.clone());
        Harness::new(Mode::Plain, (), move |_: &mut ()| {
            any(indexed_stack(Some(tab.get()))
                .child(Column((
                    Counted {
                        tag: "hidden",
                        height: 30.0,
                    },
                    scroll_view(Column(rows(20, 40.0))),
                )))
                .child(Counted {
                    tag: "shown",
                    height: height.get(),
                }))
        })
    };
    tab.set(1);
    h.frames(2);
    let (hidden, shown) = (layouts_of("hidden"), layouts_of("shown"));
    for frame in 0..20 {
        height.set(21.0 + f64::from(frame));
        h.frame();
    }
    assert_eq!(
        layouts_of("shown"),
        shown + 20,
        "the shown tab lays out once per signalled change"
    );
    assert_eq!(
        layouts_of("hidden"),
        hidden,
        "the hidden tab is skipped while the shown tab changes"
    );
    // Revealing the unchanged tab reuses its cached layout too.
    tab.set(0);
    h.frames(3);
    assert_eq!(layouts_of("hidden"), hidden);
}
