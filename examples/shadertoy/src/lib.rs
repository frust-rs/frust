//! Shadertoy — a Frust shader showcase: WGSL fragment-shader ports painted
//! full-screen through the [`shader_view::ShaderView`] escape-hatch widget,
//! behind a menu picker and an FPS HUD in the same style as
//! `benchmarks/frust_bench`'s S1 scenario.
//!
//! See [`shaders`] for the WGSL sources/registry and [`shader_view`] for the
//! animated canvas widget; this module is only the shell — app state, the
//! menu/running screens, and the [`frust::app!`] entry binding all three
//! platforms.
//!
//! Run it with `cargo run` (desktop preview) or `frust run` (Android/iOS).

pub mod shader_view;
pub mod shaders;

use frust::{
    AnyView, Axis, Brightness, Color, Component, EdgeInsets, FlexView, Get, Padding, RwSignal,
    SizedBox, Stack, SystemUiMode, Theme, any, button, flexible, inflexible, safe_area,
    set_app_theme, set_system_ui_mode, text,
};
use frust_scene::ShaderProgram;

use shader_view::shader_view;

/// The FPS readout's traffic-light thresholds — same values as
/// `benchmarks/frust_bench`'s S1 scenario HUD.
const FPS_RED_BELOW: f64 = 30.0;
const FPS_ORANGE_BELOW: f64 = 55.0;

/// Which screen is showing: the shader picker, or a running shader (by index
/// into `AppState::shaders`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Menu,
    Running(usize),
}

/// Application state: the current screen, the shader registry (built once at
/// mount), and the FPS signal the running [`shader_view::ShaderView`] writes
/// from its paint pass.
pub struct AppState {
    screen: Screen,
    shaders: Vec<(&'static str, ShaderProgram)>,
    /// Measured painted-frames-per-second, written by the shader widget
    /// ~1×/second while a shader is running.
    fps: RwSignal<f64>,
}

/// The FPS traffic-light color for a measured rate — red below 30, orange
/// below 55, green otherwise (matches `benchmarks/frust_bench`'s S1 scenario).
fn fps_color(fps: f64) -> Color {
    if fps > 0.0 && fps < FPS_RED_BELOW {
        Color::from_rgb8(0xEF, 0x53, 0x50)
    } else if fps > 0.0 && fps < FPS_ORANGE_BELOW {
        Color::from_rgb8(0xFB, 0x8C, 0x00)
    } else {
        Color::from_rgb8(0x66, 0xBB, 0x6A)
    }
}

/// The menu screen: a title plus one button per registered shader.
///
/// System UI: the menu was never meant to be immersive — bars stay
/// visible here (`EdgeToEdge`), so the screen is wrapped in [`safe_area`] to
/// keep its content clear of them. Entering a shader flips to
/// `ImmersiveSticky` (see `running_screen`'s back button, which restores
/// `EdgeToEdge` on the way out).
fn menu_screen(state: &AppState) -> AnyView<AppState> {
    let title = text("Frust Shader Showcase").size(28.0);

    let mut rows = vec![inflexible(title), inflexible(SizedBox(None, Some(24.0)))];
    for (idx, (name, _)) in state.shaders.iter().enumerate() {
        rows.push(inflexible(button(*name, move |state: &mut AppState| {
            state.screen = Screen::Running(idx);
            // Entering the shader view: go immersive-sticky (an edge swipe
            // reveals the bars transiently, then they auto-hide again).
            set_system_ui_mode(SystemUiMode::ImmersiveSticky);
        })));
        rows.push(inflexible(SizedBox(None, Some(12.0))));
    }

    let hint = text("Pick a shader — it animates fullscreen with a live FPS readout.")
        .size(12.0)
        .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xB3));
    rows.push(inflexible(hint));

    let menu = Padding(EdgeInsets::all(24.0), FlexView::new(Axis::Vertical, rows));
    any(safe_area(menu))
}

/// The running screen: the shader canvas filling the window, under a HUD
/// overlay (back button + shader name + FPS traffic-light readout at the
/// top, a hint caption at the bottom).
fn running_screen(state: &mut AppState, idx: usize) -> AnyView<AppState> {
    let (name, program) = &state.shaders[idx];
    let name = *name;
    let program = program.clone();

    let fps = state.fps.get();
    let fps_label = if fps > 0.0 {
        format!("FPS: {fps:.1}")
    } else {
        "FPS: —".to_string()
    };

    let top_row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(button("< Back", |state: &mut AppState| {
                state.screen = Screen::Menu;
                // Restore the menu's edge-to-edge (bars-visible) system UI on
                // the way out — the shader view's `ImmersiveSticky` (see
                // `menu_screen`) must not leak into the menu screen.
                set_system_ui_mode(SystemUiMode::EdgeToEdge);
            })),
            inflexible(SizedBox(Some(12.0), None)),
            inflexible(text(name).size(18.0)),
            flexible(1, SizedBox(None, None)),
            inflexible(text(fps_label).size(18.0).color(fps_color(fps))),
        ],
    );

    let hint =
        text("Fragment shader rendered offscreen and composited via vello's texture override.")
            .size(12.0)
            .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xB3));

    let hud = Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(top_row),
                flexible(1, SizedBox(None, None)),
                inflexible(hint),
            ],
        ),
    );

    any(Stack(vec![any(shader_view(program, state.fps)), any(hud)]))
}

/// The showcase's view: the menu picker, or a running shader under its HUD.
fn app_logic(state: &mut AppState) -> AnyView<AppState> {
    match state.screen {
        Screen::Menu => menu_screen(state),
        Screen::Running(idx) => running_screen(state, idx),
    }
}

/// The root [`Component`]: builds the shader registry once and
/// forces the dark M3 theme, matching `benchmarks/frust_bench`'s shell.
#[derive(Default)]
pub struct ShadertoyApp;

impl Component for ShadertoyApp {
    type State = AppState;

    fn init(&self) -> AppState {
        let mut theme = Theme::m3_baseline();
        theme.brightness = Brightness::Dark;
        set_app_theme(theme);
        AppState {
            screen: Screen::Menu,
            shaders: shaders::all(),
            fps: RwSignal::new(0.0),
        }
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        app_logic(state)
    }
}

// The generated app's sole entry point: one line binds
// `ShadertoyApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (unconditional;
// self-gated to `target_os = "ios"`), and (on desktop) the hidden
// `__frust_main` that `main.rs` calls.
frust::app!(ShadertoyApp);
