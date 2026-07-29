//! Headless integration tests: drive the real `RenderRoot`
//! rebuild→layout→paint seam against a recording paint target — no GPU, no
//! window — mirroring `examples/huddle`'s own headless `tests/*.rs` harness
//! shape.

use std::any::Any;

use frust::{AnyView, GetUntracked, RwSignal, any, component};
use frust_core::{
    FrameTime, InputEvent, PaintScene, PointerButton, PointerEvent, PointerPhase, RenderRoot, View,
};
use frust_reactive::ReactiveRuntime;
use frust_scene::{GlyphRun, ShaderProgram};
use frust_text::TextContext;
use kurbo::{Point, Rect, Size};
use peniko::{Brush, Color};
use reactive_graph::owner::Owner;

use shadertoy::ShadertoyApp;
use shadertoy::shader_view::shader_view;

const W: f64 = 800.0;
const H: f64 = 600.0;

/// A GPU-free paint target recording the showcase's workload: shader-quad
/// draws, HUD glyph runs, and button chrome (rounded rects, used to locate
/// and tap the menu/back buttons).
#[derive(Default)]
struct RecScene {
    glyph_runs: usize,
    rounded: Vec<(Point, Size)>,
    quads: Vec<(u64, Rect, f32)>,
}

impl PaintScene for RecScene {
    fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
    fn draw_text(&mut self, _origin: Point, _text: &str) {}
    fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
        self.rounded.push((origin, size));
    }
    fn fill_rounded_rect_brush(&mut self, origin: Point, size: Size, _radius: f64, _brush: &Brush) {
        self.rounded.push((origin, size));
    }
    fn draw_glyph_run(&mut self, _run: GlyphRun) {
        self.glyph_runs += 1;
    }
    fn draw_shader(&mut self, program: &ShaderProgram, dest: Rect, time: f32) {
        self.quads.push((program.id(), dest, time));
    }
}

/// Installs the reactive runtime and an ambient owner, mirroring the desktop
/// shell's startup (required before creating signals / mounting a Component).
fn setup() -> Owner {
    let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
    let ambient = Owner::new();
    ambient.set();
    ambient
}

/// One frame at `t_ms` on a caller-advanced clock: rebuild, layout (shaping
/// real text), paint into a fresh recorder. Returns the scene and whether the
/// paint asked for another frame.
fn frame_at<S: 'static, V: View<S>>(
    root: &mut RenderRoot<S, V>,
    logic: &mut impl FnMut(&mut S) -> V,
    state: &mut S,
    tcx: &mut TextContext,
    t_ms: u64,
) -> (RecScene, bool) {
    root.rebuild(logic, state);
    let tcx_any: &mut dyn Any = tcx;
    root.layout_with_text(Size::new(W, H), tcx_any);
    let mut scene = RecScene::default();
    let outcome = root.paint(&mut scene, FrameTime::from_nanos(t_ms * 1_000_000));
    (scene, outcome.needs_frame)
}

fn pointer(phase: PointerPhase, p: Point) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        phase,
        position: p,
        button: PointerButton::Primary,
    })
}

/// Tap (down + up) the center of a recorded rounded-rect chrome entry.
fn tap<S: 'static, V: View<S>>(root: &mut RenderRoot<S, V>, state: &mut S, rect: (Point, Size)) {
    let center = Point::new(
        rect.0.x + rect.1.width / 2.0,
        rect.0.y + rect.1.height / 2.0,
    );
    root.event(state, &pointer(PointerPhase::Down, center));
    root.event(state, &pointer(PointerPhase::Up, center));
}

type AppRoot = RenderRoot<(), AnyView<()>>;

fn mounted_app() -> (Owner, AppRoot, (), TextContext) {
    let owner = setup();
    let root: AppRoot = RenderRoot::new();
    (owner, root, (), TextContext::new())
}

fn app_logic(_s: &mut ()) -> AnyView<()> {
    any(component(ShadertoyApp))
}

#[test]
fn running_screen_paints_exactly_one_shader_quad_and_hud_text() {
    let (_owner, mut root, mut state, mut tcx) = mounted_app();
    let (menu, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 0);
    assert_eq!(menu.quads.len(), 0, "the menu never paints a shader quad");
    assert_eq!(menu.rounded.len(), 4, "one button per registered shader");

    // Tap the first shader button to enter the running screen.
    tap(&mut root, &mut state, menu.rounded[0]);

    let (running, needs_frame) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 16);
    assert_eq!(
        running.quads.len(),
        1,
        "the running screen paints exactly one shader quad"
    );
    let (_, dest, _) = running.quads[0];
    assert_eq!(
        dest,
        Rect::new(0.0, 0.0, W, H),
        "the quad covers the full canvas"
    );
    assert!(
        running.glyph_runs > 0,
        "the HUD paints shaped text every frame"
    );
    assert!(needs_frame, "a running shader keeps requesting frames");
}

#[test]
fn needs_frame_is_true_while_running_and_false_on_menu() {
    let (_owner, mut root, mut state, mut tcx) = mounted_app();
    let (menu, menu_needs_frame) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 0);
    assert!(
        !menu_needs_frame,
        "the static menu never requests another frame"
    );

    tap(&mut root, &mut state, menu.rounded[0]);
    let (_, running_needs_frame) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 16);
    assert!(
        running_needs_frame,
        "a running shader continuously requests frames"
    );
}

#[test]
fn switching_shaders_swaps_the_recorded_program_id() {
    let (_owner, mut root, mut state, mut tcx) = mounted_app();
    let (menu, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 0);
    assert_eq!(menu.rounded.len(), 4, "four shaders are registered");

    // Run the first shader.
    tap(&mut root, &mut state, menu.rounded[0]);
    let (first_run, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 16);
    let first_id = first_run.quads[0].0;

    // Back to the menu, then run the second shader.
    let back_button = first_run.rounded[0];
    tap(&mut root, &mut state, back_button);
    let (back_to_menu, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 32);
    assert_eq!(
        back_to_menu.quads.len(),
        0,
        "back returns to the menu — no quad recorded"
    );

    tap(&mut root, &mut state, back_to_menu.rounded[1]);
    let (second_run, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 48);
    let second_id = second_run.quads[0].0;

    assert_ne!(
        first_id, second_id,
        "switching shaders swaps the recorded program id"
    );
}

#[test]
fn back_button_returns_to_menu_with_no_quad_recorded() {
    let (_owner, mut root, mut state, mut tcx) = mounted_app();
    let (menu, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 0);
    tap(&mut root, &mut state, menu.rounded[0]);
    let (running, _) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 16);
    assert_eq!(running.quads.len(), 1);

    let back_button = running.rounded[0];
    tap(&mut root, &mut state, back_button);
    let (back_to_menu, needs_frame) = frame_at(&mut root, &mut app_logic, &mut state, &mut tcx, 32);
    assert_eq!(back_to_menu.quads.len(), 0, "back returns to the menu");
    assert_eq!(
        back_to_menu.rounded.len(),
        4,
        "the menu's four shader buttons repaint"
    );
    assert!(!needs_frame, "the menu is static once more");
}

#[test]
fn fps_signal_publishes_a_sane_rate_after_a_second() {
    let _owner = setup();
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let fps = RwSignal::new(0.0_f64);
    let program = ShaderProgram::new(
        "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> \
         { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }",
    );
    let mut logic = move |_s: &mut ()| any(shader_view(program.clone(), fps));
    let mut state = ();
    let mut tcx = TextContext::new();

    // ~1.2 simulated seconds at 60Hz. `frame_at` never wires a presented
    // counter, so this exercises the paint-count fallback path (~60 FPS).
    for i in 0..75u64 {
        let _ = frame_at(&mut root, &mut logic, &mut state, &mut tcx, i * 16);
    }
    let measured = fps.get_untracked();
    assert!(
        (30.0..=120.0).contains(&measured),
        "measured FPS must reflect the 16ms frame cadence (got {measured})"
    );
}

#[test]
fn fps_measures_presented_rate_not_paint_cadence_when_wired() {
    // The OP9 split scenario (task 10): the UI thread paints ~120 frames/s but
    // the render thread presents only ~12 — the HUD must report the presented
    // rate (~12), NOT the paint cadence (the deferred round-1 Minor: it showed
    // ~121). We drive the real `RenderRoot` paint seam, pushing a presented
    // counter that advances once per 10 paints, and assert the published rate
    // tracks the presented deltas.
    let _owner = setup();
    let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
    let fps = RwSignal::new(0.0_f64);
    let program = ShaderProgram::new(
        "@fragment fn fs_main(in: FrustVsOut) -> @location(0) vec4<f32> \
         { return vec4<f32>(frust_u.time, 0.0, 0.0, 1.0); }",
    );
    let mut logic = move |_s: &mut ()| any(shader_view(program.clone(), fps));
    let mut state = ();
    let mut tcx = TextContext::new();

    // 121 painted frames spanning ~1.008s of shell time (~120Hz paint cadence),
    // of which the render thread presented only 12 (one present per 10 paints).
    let mut presented: u64 = 0;
    for i in 0..=120u64 {
        root.rebuild(&mut logic, &mut state);
        let tcx_any: &mut dyn Any = &mut tcx;
        root.layout_with_text(Size::new(W, H), tcx_any);
        // Every 10th paint corresponds to one presented frame (=> 12 total).
        if i > 0 && i % 10 == 0 {
            presented += 1;
        }
        root.set_presented_frames(presented);
        let mut scene = RecScene::default();
        // ~8.4ms/frame => the window crosses 1s exactly at frame 120.
        let _ = root.paint(&mut scene, FrameTime::from_nanos(i * 8_400_000));
    }

    let measured = fps.get_untracked();
    assert!(
        (10.0..=14.0).contains(&measured),
        "wired HUD must measure the ~12 presented rate, not the ~120 paint \
         cadence (got {measured})"
    );
}
