//! `custom-titlebar`: the reference for an app-drawn title bar on every desktop OS.
//!
//! Three shapes, one component tree:
//! - macOS: `Decorations::NoTitleBar`. AppKit keeps the traffic lights (placed by
//!   `TitleButtons`) and the system edge resize; the app draws only the bar.
//! - Windows / Linux: `Decorations::None`. The frame is gone, so the bar carries
//!   its own Minimize / Maximize / Close buttons and eight resize strips ring
//!   the content (hidden while maximized or fullscreen).
//!
//! Single binary, no `lib.rs`: `main` calls the generated `__frust_main`.

use frust::authoring::Size;
use frust::{
    Align, Alignment, ButtonStyle, Color, Component, Decorations, DesktopConfig, Get, ResizeEdge,
    RwSignal, SizedBox, Stack, Theme, TitleButtons, Update, View, WindowCommand, WindowState,
    button, colored_box, column, push_window_command, row, text, use_context, window_drag_region,
};

/// Logical height of the bar; also the macOS traffic-light vertical extent.
const BAR_HEIGHT: f64 = 40.0;
/// Space reserved for the macOS traffic lights (leading 16 + three buttons + gaps).
const TRAFFIC_LIGHT_WIDTH: f64 = 80.0;
/// Thickness of an edge strip and side of a corner square.
const EDGE: f64 = 6.0;
const CORNER: f64 = 10.0;

const IS_MACOS: bool = cfg!(target_os = "macos");

#[derive(Default)]
struct CustomTitleBarApp;

struct AppState {
    count: RwSignal<i32>,
}

impl Component for CustomTitleBarApp {
    type State = AppState;

    fn init(&self) -> AppState {
        AppState {
            count: RwSignal::new(0),
        }
    }

    fn build(&self, state: &mut AppState) -> impl View<AppState> {
        let window = use_context::<WindowState>();
        let count = state.count;

        let content = column()
            .child(Self::title_bar(window))
            .flex(1, Self::body(count, window));

        Self::resize_frame(content, window)
    }
}

impl CustomTitleBarApp {
    fn surface() -> Color {
        use_context::<Theme>()
            .unwrap_or_else(Theme::neutral)
            .scheme()
            .surface_container
    }

    fn title_bar(window: Option<WindowState>) -> impl View<AppState> {
        let title = text("Custom Title Bar").size(14.0);
        let controls = row()
            .child(Self::window_button("Minimize", WindowCommand::Minimize))
            .child(Self::window_button(
                if window.is_some_and(|w| w.maximized) {
                    "Restore"
                } else {
                    "Maximize"
                },
                WindowCommand::ToggleMaximized,
            ))
            .child(Self::window_button("Close", WindowCommand::RequestClose));

        let bar = row()
            .cross_axis(frust::CrossAxisAlignment::Center)
            .when(IS_MACOS, |r| {
                r.child(SizedBox(Some(TRAFFIC_LIGHT_WIDTH), None))
            })
            .child(title)
            .when(!IS_MACOS, |r| {
                r.flex(1, SizedBox(None, None)).child(controls)
            });

        window_drag_region(
            SizedBox(None, Some(BAR_HEIGHT))
                .child(Stack((colored_box().fill(Self::surface()).expand(), bar))),
        )
    }

    fn window_button(label: &'static str, command: WindowCommand) -> impl View<AppState> {
        button(label, move |_: &mut AppState| push_window_command(command))
            .style(ButtonStyle::Ghost)
            .small()
    }

    fn body(count: RwSignal<i32>, window: Option<WindowState>) -> impl View<AppState> {
        let flag = |on: fn(&WindowState) -> bool| match window {
            Some(w) => on(&w).to_string(),
            None => "-".to_string(),
        };
        let readout = format!(
            "maximized {} / fullscreen {} / focused {} / minimized {}",
            flag(|w| w.maximized),
            flag(|w| w.fullscreen),
            flag(|w| w.focused),
            flag(|w| w.minimized),
        );
        let fullscreen = window.is_some_and(|w| w.fullscreen);

        column()
            .child(text(format!("Count: {}", count.get())).size(24.0))
            .child(button("+", move |_: &mut AppState| {
                count.update(|c| *c += 1)
            }))
            .child(text(readout).size(12.0))
            .child(button("Toggle fullscreen", move |_: &mut AppState| {
                push_window_command(WindowCommand::SetFullscreen(!fullscreen))
            }))
    }

    /// Ring `content` with resize strips; macOS and maximized/fullscreen windows
    /// get the content unchanged (system edge resize / nothing to resize).
    fn resize_frame(
        content: impl View<AppState>,
        window: Option<WindowState>,
    ) -> impl View<AppState> {
        let hidden = IS_MACOS || window.is_some_and(|w| w.maximized || w.fullscreen);
        let strips = (!hidden).then(|| {
            let edge = |a: Alignment, w: Option<f64>, h: Option<f64>, e: ResizeEdge| {
                Align(a, window_drag_region(SizedBox(w, h)).resize(e))
            };
            Stack((
                edge(
                    Alignment::new(0.0, -1.0),
                    None,
                    Some(EDGE),
                    ResizeEdge::North,
                ),
                edge(
                    Alignment::new(0.0, 1.0),
                    None,
                    Some(EDGE),
                    ResizeEdge::South,
                ),
                edge(
                    Alignment::new(-1.0, 0.0),
                    Some(EDGE),
                    None,
                    ResizeEdge::West,
                ),
                edge(Alignment::new(1.0, 0.0), Some(EDGE), None, ResizeEdge::East),
                edge(
                    Alignment::TOP_LEFT,
                    Some(CORNER),
                    Some(CORNER),
                    ResizeEdge::NorthWest,
                ),
                edge(
                    Alignment::TOP_RIGHT,
                    Some(CORNER),
                    Some(CORNER),
                    ResizeEdge::NorthEast,
                ),
                edge(
                    Alignment::BOTTOM_LEFT,
                    Some(CORNER),
                    Some(CORNER),
                    ResizeEdge::SouthWest,
                ),
                edge(
                    Alignment::BOTTOM_RIGHT,
                    Some(CORNER),
                    Some(CORNER),
                    ResizeEdge::SouthEast,
                ),
            ))
        });
        Stack((content, strips))
    }
}

frust::app!(
    CustomTitleBarApp,
    desktop = {
        DesktopConfig::new()
            .with_app_name("Custom Title Bar")
            .with_decorations(if cfg!(target_os = "macos") {
                Decorations::NoTitleBar
            } else {
                Decorations::None
            })
            .with_title_buttons(TitleButtons::new(16.0, 40.0))
            .with_min_size(Size::new(480.0, 320.0))
    }
);

fn main() {
    __frust_main();
}
