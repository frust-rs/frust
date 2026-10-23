//! Desktop section: native file dialogs (`frust-file-dialogs`) and the desktop
//! window services (`DesktopConfig` min/max size and window-state
//! persistence), exercised from real button presses.
//!
//! # File dialogs
//!
//! "Open file", "Open files" and "Save as\u{2026}" each run the blocking dialog
//! inside [`frust::spawn_blocking`], delivered through [`use_task`]: the
//! result lands in an [`AsyncValue`] written on the UI thread by the task
//! coordinator (the page never sets a signal from the background thread). The
//! state reads `Loading` while the dialog is up. Only file *names* are listed
//! (no contents, no full paths).
//!
//! `use_task` starts a first fetch at mount, so the fetch closure reads a
//! shared request cell: [`Request::None`] resolves immediately to an empty
//! list, a button press stores its request and calls `restart`.
//!
//! On targets without a dialog backend the plugin returns
//! [`FileDialogError::Unsupported`]; the page then shows "desktop only"
//! (the buttons stay in place and call the plugin — no `cfg` split).
//!
//! # Window card
//!
//! Shows the live logical window size from [`WindowMetrics`] and where the
//! persisted state file lives. The matching `DesktopConfig` is set in
//! `lib.rs`'s `frust::app!` call.
//!
//! - The Window row's Minimize, Maximize/Restore and Enter/Exit fullscreen
//!   buttons push a [`WindowCommand`]; the readout follows the live
//!   [`WindowState`] (or says there is none off desktop).

use std::fmt;
use std::sync::{Arc, Mutex};

use frust::{
    AsyncValue, ButtonStyle, Color, Component, EdgeInsets, Get, Padding, SizedBox, Theme, UseTask,
    View, WindowCommand, WindowMetrics, WindowState, button, column, component,
    push_window_command, row, spawn_blocking, text, use_context, use_task,
};
use frust_file_dialogs::{FileDialog, FileDialogError, FileFilter};

use crate::PlaygroundState;

/// Which dialog a button press asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    /// The mount-time fetch: nothing asked yet.
    None,
    OpenFile,
    OpenFiles,
    SaveAs,
}

/// The task's error: a dialog failure or a join failure, as display text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PageError {
    /// The target has no dialog backend.
    DesktopOnly,
    Other(String),
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageError::DesktopOnly => f.write_str("desktop only"),
            PageError::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for PageError {}

impl From<FileDialogError> for PageError {
    fn from(err: FileDialogError) -> Self {
        match err {
            FileDialogError::Unsupported => PageError::DesktopOnly,
            other => PageError::Other(other.to_string()),
        }
    }
}

/// Final path component of each picked path; the full path is never shown.
fn names_of(paths: impl IntoIterator<Item = std::path::PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "(unnamed)".to_string())
        })
        .collect()
}

/// Runs the blocking dialog for `request` (call inside `spawn_blocking`).
fn run_dialog(request: Request) -> Result<Vec<String>, PageError> {
    let dialog = FileDialog::new().filter(FileFilter::new("Images", &["png", "jpg", "jpeg"]));
    Ok(match request {
        Request::None => Vec::new(),
        Request::OpenFile => names_of(dialog.title("Open file").open_file()?),
        Request::OpenFiles => names_of(dialog.title("Open files").open_files()?),
        Request::SaveAs => names_of(
            FileDialog::new()
                .title("Save as")
                .save_file(Some("frust-playground.txt"))?,
        ),
    })
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(DesktopPage)
}

struct DesktopPage;

struct DesktopPageState {
    picked: UseTask<Vec<String>>,
    request: Arc<Mutex<Request>>,
}

impl Component for DesktopPage {
    type State = DesktopPageState;

    fn init(&self) -> DesktopPageState {
        let request = Arc::new(Mutex::new(Request::None));
        let cell = request.clone();
        let picked = use_task(move || {
            let request = *cell.lock().expect("request cell poisoned");
            async move {
                match spawn_blocking(move || run_dialog(request)).await {
                    Ok(result) => result,
                    Err(join_err) => Err(PageError::Other(join_err.to_string())),
                }
            }
        });
        DesktopPageState { picked, request }
    }

    fn build(&self, state: &mut DesktopPageState) -> impl View<DesktopPageState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let (status, names) = status_lines(&state.picked.signal().get());
        let window = use_context::<WindowMetrics>()
            .map(|m| {
                format!(
                    "Window: {:.0} x {:.0} logical px",
                    m.size.width, m.size.height
                )
            })
            .unwrap_or_else(|| "Window: size not published".to_string());
        let window_state = use_context::<WindowState>();
        let maximized = window_state.is_some_and(|s| s.maximized);
        let fullscreen = window_state.is_some_and(|s| s.fullscreen);
        let readout = match window_state {
            Some(s) => format!(
                "maximized={} fullscreen={} focused={} minimized={}",
                s.maximized, s.fullscreen, s.focused, s.minimized
            ),
            None => "WindowState: none (not a desktop shell)".to_string(),
        };

        let mut list = column();
        for name in names {
            list = list.child(text(name).size(12.0));
        }

        Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Desktop").size(13.0).color(accent))
                .child(
                    text(
                        "Native file dialogs run off the UI thread and report file names \
                         only. The window below is configured with a minimum size and \
                         persisted geometry (see DesktopConfig in lib.rs).",
                    )
                    .size(11.0)
                    .color(muted),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(
                    row()
                        .child(dialog_button(state, "Open file", Request::OpenFile))
                        .child(SizedBox(Some(8.0), None))
                        .child(dialog_button(state, "Open files", Request::OpenFiles))
                        .child(SizedBox(Some(8.0), None))
                        .child(dialog_button(state, "Save as\u{2026}", Request::SaveAs)),
                )
                .child(SizedBox(None, Some(8.0)))
                .child(text(status).size(12.0).color(status_color(muted)))
                .child(list)
                .child(SizedBox(None, Some(16.0)))
                .child(text("Window").size(13.0).color(accent))
                .child(text(window).size(12.0))
                .child(
                    row()
                        .child(window_button("Minimize", WindowCommand::Minimize))
                        .child(SizedBox(Some(8.0), None))
                        .child(window_button(
                            if maximized { "Restore" } else { "Maximize" },
                            WindowCommand::ToggleMaximized,
                        ))
                        .child(SizedBox(Some(8.0), None))
                        .child(window_button(
                            if fullscreen {
                                "Exit fullscreen"
                            } else {
                                "Enter fullscreen"
                            },
                            WindowCommand::SetFullscreen(!fullscreen),
                        )),
                )
                .child(text(readout).size(12.0).color(muted))
                .child(
                    text(
                        "Min size 480 x 320; geometry is saved to \
                         <data dir>/frust/window_state_<app>.txt and restored on the \
                         next launch.",
                    )
                    .size(11.0)
                    .color(muted),
                ),
        )
    }
}

fn status_color(muted: Color) -> Color {
    muted
}

fn dialog_button(
    state: &DesktopPageState,
    label: &'static str,
    request: Request,
) -> impl View<DesktopPageState> {
    let picked = state.picked.clone();
    let cell = state.request.clone();
    button(label, move |_state: &mut DesktopPageState| {
        *cell.lock().expect("request cell poisoned") = request;
        picked.restart();
    })
    .style(ButtonStyle::Secondary)
    .small()
}

/// A window-command button: a press queues `command` for the desktop shell.
fn window_button(label: &'static str, command: WindowCommand) -> impl View<DesktopPageState> {
    button(label, move |_state: &mut DesktopPageState| {
        push_window_command(command)
    })
    .style(ButtonStyle::Secondary)
    .small()
}

/// A status line plus the names to list, for each task state.
fn status_lines(value: &AsyncValue<Vec<String>>) -> (String, Vec<String>) {
    match value {
        AsyncValue::Idle => ("Nothing picked yet.".to_string(), Vec::new()),
        AsyncValue::Loading(_) => ("Dialog open\u{2026}".to_string(), Vec::new()),
        AsyncValue::Ready(names) if names.is_empty() => (
            "Nothing picked (cancelled or not asked yet).".to_string(),
            Vec::new(),
        ),
        AsyncValue::Ready(names) => (format!("Picked {} name(s):", names.len()), names.clone()),
        AsyncValue::Error(err) => (err.to_string(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn names_only_no_directories() {
        let names = names_of([PathBuf::from("/a/b/c.png"), PathBuf::from("d.jpg")]);
        assert_eq!(names, vec!["c.png".to_string(), "d.jpg".to_string()]);
    }

    #[test]
    fn unsupported_maps_to_desktop_only() {
        assert_eq!(
            PageError::from(FileDialogError::Unsupported).to_string(),
            "desktop only"
        );
    }

    #[test]
    fn no_request_resolves_empty() {
        assert_eq!(run_dialog(Request::None), Ok(Vec::new()));
    }
}
