//! Shadertoy — a Frust shader showcase: WGSL fragment-shader ports painted
//! full-screen through the hand-rolled [`shader_view::ShaderView`] widget,
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

use frust::authoring::scene::ShaderProgram;
use frust::{
    Axis, Brightness, Color, Component, EdgeInsets, Either, FlexView, Get, Padding, RwSignal,
    SizedBox, SystemUiMode, View, button, column, inflexible, row, safe_area, set_app_theme,
    set_system_ui_mode, stack, text,
};

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
fn menu_screen(state: &AppState) -> impl View<AppState> {
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
    safe_area(menu)
}

/// The running screen's HUD: back button + shader name + FPS traffic-light
/// readout at the top, a hint caption at the bottom.
fn hud_overlay(name: &'static str, fps: f64) -> impl frust::View<AppState> {
    let fps_label = if fps > 0.0 {
        format!("FPS: {fps:.1}")
    } else {
        "FPS: —".to_string()
    };

    let top_row = row()
        .child(button("< Back", |state: &mut AppState| {
            state.screen = Screen::Menu;
            // Restore the menu's edge-to-edge (bars-visible) system UI on
            // the way out — the shader view's `ImmersiveSticky` (see
            // `menu_screen`) must not leak into the menu screen.
            set_system_ui_mode(SystemUiMode::EdgeToEdge);
        }))
        .child(SizedBox(Some(12.0), None))
        .child(text(name).size(18.0))
        .flex(1, SizedBox(None, None))
        .child(text(fps_label).size(18.0).color(fps_color(fps)));

    let hint =
        text("Fragment shader rendered offscreen by the engine's ShaderQuad pass and drawn as a SceneTexture.")
            .size(12.0)
            .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xB3));

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(top_row)
            .flex(1, SizedBox(None, None))
            .child(hint),
    )
}

/// The running screen: the shader canvas filling the window, under a HUD
/// overlay (back button + shader name + FPS traffic-light readout at the
/// top, a hint caption at the bottom).
fn running_screen(state: &mut AppState, idx: usize) -> impl View<AppState> {
    let (name, program) = &state.shaders[idx];
    let name = *name;
    let program = program.clone();

    let hud = hud_overlay(name, state.fps.get());

    stack().child(shader_view(program, state.fps)).child(hud)
}

/// The root [`Component`]: builds the shader registry once and
/// forces the dark M3 theme, matching `benchmarks/frust_bench`'s shell.
#[derive(Default)]
pub struct ShadertoyApp;

impl Component for ShadertoyApp {
    type State = AppState;

    fn init(&self) -> AppState {
        let mut theme = frust_material::baseline();
        theme.brightness = Brightness::Dark;
        set_app_theme(theme);
        AppState {
            screen: Screen::Menu,
            shaders: shaders::all(),
            fps: RwSignal::new(0.0),
        }
    }

    fn build(&self, state: &mut AppState) -> impl View<AppState> {
        match state.screen {
            Screen::Menu => Either::Left(menu_screen(state)),
            Screen::Running(idx) => Either::Right(running_screen(state, idx)),
        }
    }
}

// The generated app's sole entry point: one line binds
// `ShadertoyApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (unconditional;
// self-gated to `target_os = "ios"`), and (on desktop) the hidden
// `__frust_main` that `main.rs` calls.
frust::app!(ShadertoyApp);
