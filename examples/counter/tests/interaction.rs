//! Headless interaction proof for the counter demo.
//!
//! Desktop UI automation (osascript/cliclick) is unavailable on this host, so
//! rather than driving a live GPU window this test feeds synthetic
//! [`InputEvent`]s straight into the framework's [`RenderRoot`] — the exact
//! event seam the desktop shell's `dispatch` uses (`RenderRoot::event`) and the
//! mobile shells reach through `AppTree::event`. Text is shaped through a real
//! `TextContext` (CPU-only, no GPU) and paint output is recorded, so every
//! assertion is deterministic and this doubles as a permanent regression gate
//! for the counter's event wiring.
//!
//! Coverage: +/− buttons change the count; the checkbox toggle grows the
//! rebuild-driven filler list; a horizontal slider drag updates its value; a
//! wheel notch scrolls the list; and — the headline disambiguation — a drag
//! that *starts on a button* scrolls the list instead of firing the button.
//! `local_section_state_survives_parent_rebuilds` proves the phase 5.5
//! `Component` model's headline feature: the nested `CollapsibleSection`'s own
//! local state is untouched by a parent-driven rebuild.

use std::any::Any;

use counter::{AppState, BASE_ROWS, CounterApp, EXTRA_ROWS};
use forgekit_core::{
    Component, InputEvent, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot,
    ScrollDelta, View,
};
use forgekit_scene::GlyphRun;
use forgekit_text::TextContext;
use kurbo::{Point, Size};
use peniko::Color;

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target: records rounded-rect geometry (buttons, checkbox
/// box, slider track/thumb) and counts glyph runs (one-ish per visible text
/// row) so the test can locate widgets and observe list length / scroll shift.
#[derive(Default)]
struct RecScene {
    rounded: Vec<(Point, Size)>,
    glyph_runs: usize,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
        self.rounded.push((origin, size));
    }
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
}

/// One frame: rebuild the tree from `state`, lay it out at window size, and
/// record the paint output.
fn frame<V: View<AppState>>(
    root: &mut RenderRoot<AppState, V>,
    logic: &mut impl FnMut(&mut AppState) -> V,
    state: &mut AppState,
    tcx: &mut TextContext,
) -> RecScene {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    paint(root)
}

/// Re-paint without rebuilding/laying out — used to observe an event's effect
/// (e.g. a scroll offset change) without a state round-trip.
fn paint<V: View<AppState>>(root: &mut RenderRoot<AppState, V>) -> RecScene {
    let mut scene = RecScene::default();
    root.paint(&mut scene);
    scene
}

fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: Point::new(x, y),
        button: PointerButton::Primary,
    })
}

/// The center of a recorded rect.
fn center((origin, size): (Point, Size)) -> Point {
    Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0)
}

/// The −/+ button rects: the two rounded rects sharing the smallest top-y (the
/// button `Row` sits above the checkbox and slider). Returned left-to-right, so
/// `.0` is "−" and `.1` is "+".
fn buttons(scene: &RecScene) -> ((Point, Size), (Point, Size)) {
    let min_y = scene
        .rounded
        .iter()
        .map(|(o, _)| o.y)
        .fold(f64::INFINITY, f64::min);
    let mut row: Vec<(Point, Size)> = scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, _)| (o.y - min_y).abs() < 1.0)
        .collect();
    assert_eq!(
        row.len(),
        2,
        "expected exactly two button rects in the top row, got {:?}",
        row
    );
    row.sort_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap());
    (row[0], row[1])
}

/// The checkbox box: the smallest-y small square rounded rect. The slider thumb
/// is also a small square but sits below the checkbox; the buttons are square-ish
/// but far larger (≈33px), so a `< 25px` size bound excludes them.
fn checkbox_box(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(_, s)| (s.width - s.height).abs() < 2.0 && s.width < 25.0)
        .min_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("a small square checkbox box rect")
}

/// The slider track: the widest rounded rect (it spans the viewport width,
/// dwarfing the buttons and checkbox box).
fn slider_track(scene: &RecScene) -> (Point, Size) {
    scene
        .rounded
        .iter()
        .copied()
        .max_by(|a, b| a.1.width.partial_cmp(&b.1.width).unwrap())
        .expect("a slider track rect")
}

#[test]
fn plus_and_minus_buttons_change_count() {
    let mut root = RenderRoot::new();
    let mut logic = |s: &mut AppState| CounterApp.build(s);
    let mut state = CounterApp.init();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (minus, plus) = buttons(&scene);

    // Tap "+" three times (Down then Up inside → fires on up-inside).
    for _ in 0..3 {
        let p = center(plus);
        root.event(&mut state, &pointer(PointerPhase::Down, p.x, p.y));
        root.event(&mut state, &pointer(PointerPhase::Up, p.x, p.y));
    }
    assert_eq!(state.count, 3, "three + taps should raise the count to 3");

    // Tap "−" once.
    let m = center(minus);
    root.event(&mut state, &pointer(PointerPhase::Down, m.x, m.y));
    root.event(&mut state, &pointer(PointerPhase::Up, m.x, m.y));
    assert_eq!(state.count, 2, "one − tap should drop the count to 2");
}

#[test]
fn checkbox_toggle_grows_the_filler_list() {
    let mut root = RenderRoot::new();
    let mut logic = |s: &mut AppState| CounterApp.build(s);
    let mut state = CounterApp.init();
    let mut tcx = TextContext::new();

    let before = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let box_center = center(checkbox_box(&before));
    let runs_before = before.glyph_runs;

    root.event(
        &mut state,
        &pointer(PointerPhase::Down, box_center.x, box_center.y),
    );
    root.event(
        &mut state,
        &pointer(PointerPhase::Up, box_center.x, box_center.y),
    );
    assert!(
        state.extra_rows,
        "tapping the checkbox toggles extra_rows on"
    );

    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        after.glyph_runs > runs_before,
        "toggling extra rows must grow the visible text count \
         ({runs_before} → {}); list length is a function of state \
         (BASE_ROWS={BASE_ROWS} → EXTRA_ROWS={EXTRA_ROWS})",
        after.glyph_runs
    );
}

#[test]
fn slider_drag_updates_value() {
    let mut root = RenderRoot::new();
    let mut logic = |s: &mut AppState| CounterApp.build(s);
    let mut state = CounterApp.init();
    let mut tcx = TextContext::new();

    let scene = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let (o, s) = slider_track(&scene);
    let y = o.y + s.height / 2.0;
    // Horizontal drag from near the left of the track to near the right. The y
    // stays fixed, so the ScrollView never crosses its vertical slop and the
    // slider keeps the gesture.
    let x_start = o.x + s.width * 0.1;
    let x_end = o.x + s.width * 0.9;
    root.event(&mut state, &pointer(PointerPhase::Down, x_start, y));
    root.event(&mut state, &pointer(PointerPhase::Move, x_end, y));
    root.event(&mut state, &pointer(PointerPhase::Up, x_end, y));

    assert!(
        state.slider > 0.6,
        "dragging the thumb right should raise the value well past its 0.5 \
         start, got {}",
        state.slider
    );
}

#[test]
fn wheel_scroll_shifts_content() {
    let mut root = RenderRoot::new();
    let mut logic = |s: &mut AppState| CounterApp.build(s);
    let mut state = CounterApp.init();
    let mut tcx = TextContext::new();

    let before = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let button_y_before = buttons(&before).1.0.y;

    // One wheel notch downward (positive line delta → offset grows → content
    // moves up). The 30-row list overflows the 600px viewport, so this is a
    // real scroll, not a clamped no-op.
    root.event(
        &mut state,
        &InputEvent::Scroll {
            position: Point::new(W / 2.0, H / 2.0),
            delta: ScrollDelta::Lines(0.0, 3.0),
        },
    );

    let after = paint(&mut root);
    let button_y_after = buttons(&after).1.0.y;
    assert!(
        button_y_after < button_y_before - 1.0,
        "scrolling down should shift painted content up (button y {button_y_before} → {button_y_after})"
    );
}

#[test]
fn drag_starting_on_a_button_scrolls_instead_of_firing() {
    let mut root = RenderRoot::new();
    let mut logic = |s: &mut AppState| CounterApp.build(s);
    let mut state = CounterApp.init();
    let mut tcx = TextContext::new();

    let before = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let plus = buttons(&before).1;
    let p = center(plus);
    let button_y_before = before
        .rounded
        .iter()
        .map(|(o, _)| o.y)
        .fold(f64::INFINITY, f64::min);

    // Press ON the + button, then drag upward well past the touch slop. The
    // ScrollView must take the gesture over (cancelling the armed button) and
    // scroll — the button must NOT fire on release.
    root.event(&mut state, &pointer(PointerPhase::Down, p.x, p.y));
    root.event(&mut state, &pointer(PointerPhase::Move, p.x, p.y - 40.0)); // crosses slop → takeover
    root.event(&mut state, &pointer(PointerPhase::Move, p.x, p.y - 90.0)); // applies scroll delta
    root.event(&mut state, &pointer(PointerPhase::Up, p.x, p.y - 90.0));

    assert_eq!(
        state.count, 0,
        "a drag that starts on the + button must scroll, not fire it"
    );

    let after = paint(&mut root);
    let button_y_after = after
        .rounded
        .iter()
        .map(|(o, _)| o.y)
        .fold(f64::INFINITY, f64::min);
    assert!(
        button_y_after < button_y_before - 1.0,
        "the drag should have scrolled the list up (button y {button_y_before} → {button_y_after})"
    );
}

/// The collapsible section's own toggle button: the single rounded rect below
/// the slider track. Filler rows are plain text — they paint no rounded-rect
/// chrome at all — so nothing else can appear in that region.
fn collapsible_toggle(scene: &RecScene) -> (Point, Size) {
    let (slider_origin, _) = slider_track(scene);
    scene
        .rounded
        .iter()
        .copied()
        .filter(|(o, s)| o.y > slider_origin.y + 1.0 && s.width < W / 2.0)
        .min_by(|a, b| a.0.y.partial_cmp(&b.0.y).unwrap())
        .expect("the collapsible section's toggle button rect")
}

#[test]
fn local_section_state_survives_parent_rebuilds() {
    let mut root = RenderRoot::new();
    let mut logic = |s: &mut AppState| CounterApp.build(s);
    let mut state = CounterApp.init();
    let mut tcx = TextContext::new();

    let collapsed = frame(&mut root, &mut logic, &mut state, &mut tcx);
    let runs_collapsed = collapsed.glyph_runs;

    // Expand the nested section — mutates ONLY its own local `bool` state,
    // never `AppState`.
    let toggle = center(collapsible_toggle(&collapsed));
    root.event(&mut state, &pointer(PointerPhase::Down, toggle.x, toggle.y));
    root.event(&mut state, &pointer(PointerPhase::Up, toggle.x, toggle.y));

    let expanded = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert!(
        expanded.glyph_runs > runs_collapsed,
        "expanding the section must paint its two tip lines \
         ({runs_collapsed} → {})",
        expanded.glyph_runs
    );

    // A PARENT-driven rebuild: tap "+" three times, mutating `AppState.count`
    // (not the section's own state) and re-running `CounterApp::build` — the
    // section's `ComponentView` re-runs `CollapsibleSection::build` too, but
    // its retained `state: bool` field is untouched by that re-run.
    let (_, plus) = buttons(&expanded);
    let p = center(plus);
    for _ in 0..3 {
        root.event(&mut state, &pointer(PointerPhase::Down, p.x, p.y));
        root.event(&mut state, &pointer(PointerPhase::Up, p.x, p.y));
    }
    assert_eq!(state.count, 3, "the parent-rebuild driver actually fired");

    // The section is STILL expanded after the parent rebuild: its local
    // `bool` state lives in its own retained widget, not in `AppState`, so
    // `CounterApp::build` re-running above it never resets it.
    let after = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        after.glyph_runs, expanded.glyph_runs,
        "the section's local expanded state must survive a parent-driven \
         rebuild — the glyph run count should be unchanged from the expanded \
         frame, not reset to the collapsed count ({runs_collapsed})"
    );
}
