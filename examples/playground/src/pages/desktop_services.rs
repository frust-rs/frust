//! Desktop section: native file dialogs (`frust-file-dialogs`) and the desktop
//! window services (`DesktopConfig` min/max size and window-state
//! persistence), exercised from real button presses.
//!
//! # File dialogs
//!
//! "Open file", "Open files", "Save as\u{2026}", "Pick folder" and "Export file" each run the
//! blocking dialog inside [`frust::spawn_blocking`], delivered through [`use_task`]: the
//! result lands in an [`AsyncValue`] written on the UI thread by the task
//! coordinator (the page never sets a signal from the background thread). The
//! state reads `Loading` while the dialog is up. Only file *names* are listed
//! (no contents, no full paths).
//!
//! While a call runs the status line reads `Waiting for the dialog...` until the first
//! progress report, then `Loading {done} of {items} ({pct}%)`; the buttons are disabled and
//! **Cancel** fires the call's own [`CancelToken`] (a fresh one per press). The progress
//! callback runs on a background thread, so it only writes a shared cell; the task future
//! polls that cell and copies changes into a signal (the same shape as the Photos page's
//! Picker panel).
//!
//! **Pick folder** shows the folder's name and how many files it holds (the count is a
//! cancellable listing), then releases the grant; the Photos page's Folders panel is the one
//! that keeps a folder. **Export file** writes a small generated text file into the app's
//! cache directory and hands it to `export_file`, which copies it to the place the user
//! chooses. Open and export work on iOS and Android too (open copies the file into the app);
//! `save_file` stays desktop-only.
//!
//! `use_task` starts a first fetch at mount, so the fetch closure reads a
//! shared request cell: [`Request::None`] resolves immediately to an empty
//! list, a button press stores its request and calls `restart`.
//!
//! `save_file` returns [`FileDialogError::Unsupported`] on iOS and Android; the page then
//! shows "desktop only" (the buttons stay in place and call the plugin, no `cfg` split).
//! The same holds for every call on a target with no backend at all.
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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use frust::{
    AsyncValue, ButtonStyle, Color, Component, EdgeInsets, Get, GetUntracked, Padding, RwSignal,
    Set, SizedBox, Theme, UseTask, View, WindowCommand, WindowMetrics, WindowState, button, column,
    component, push_window_command, row, spawn_blocking, text, use_context, use_task,
};
use frust_file_dialogs::{
    CancelToken, ExportOutcome, FileDialog, FileDialogError, FileFilter, FileProgress, ListOptions,
};

use crate::PlaygroundState;

/// Which dialog a button press asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    /// The mount-time fetch: nothing asked yet.
    None,
    OpenFile,
    OpenFiles,
    SaveAs,
    PickFolder,
    ExportFile,
}

/// How often the task polls the progress cell while a call runs.
const PROGRESS_POLL: Duration = Duration::from_millis(100);

/// The latest progress report, written by the callback thread.
type ProgressCell = Arc<Mutex<Option<FileProgress>>>;

/// The file name `Export file` writes and suggests.
const EXPORT_NAME: &str = "frust-playground-export.txt";

/// The task's error: a dialog failure or a join failure, as display text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PageError {
    /// The target has no dialog backend.
    DesktopOnly,
    /// The call's token fired.
    Cancelled,
    Other(String),
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageError::DesktopOnly => f.write_str("desktop only"),
            PageError::Cancelled => f.write_str("Cancelled"),
            PageError::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for PageError {}

impl From<FileDialogError> for PageError {
    fn from(err: FileDialogError) -> Self {
        match err {
            FileDialogError::Unsupported => PageError::DesktopOnly,
            FileDialogError::Cancelled => PageError::Cancelled,
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

/// A dialog builder wired to the call's cancel token and progress cell.
fn dialog(cancel: &CancelToken, sink: &ProgressCell) -> FileDialog {
    let sink = sink.clone();
    FileDialog::new()
        .on_progress(move |progress| {
            *sink.lock().expect("progress cell poisoned") = Some(progress);
        })
        .cancel(cancel.clone())
}

/// The status line while a call runs: waiting until the first report, then the count and
/// percentage. `progress` is `(done, items, fraction)`.
fn loading_text(progress: Option<(usize, usize, f64)>) -> String {
    match progress {
        None => "Waiting for the dialog\u{2026}".to_string(),
        Some((done, items, fraction)) => {
            let pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
            format!("Loading {done} of {items} ({pct}%)")
        }
    }
}

/// Where the generated export source lives: the app's cache directory, else the temp dir.
fn export_source() -> PathBuf {
    frust::paths::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("frust-file-dialogs")
        .join(EXPORT_NAME)
}

/// Writes the generated text file `Export file` hands to the dialog.
fn write_export_source() -> Result<PathBuf, PageError> {
    let path = export_source();
    let body = "Generated by the Frust playground Desktop page.\n".repeat(2048);
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, body));
    result.map(|()| path).map_err(|err| {
        log::warn!("playground: writing the export source failed: {err}");
        PageError::Other("could not write the file to export".to_string())
    })
}

/// Runs the blocking dialog for `request` (call inside `spawn_blocking`).
fn run_dialog(
    request: Request,
    cancel: &CancelToken,
    sink: &ProgressCell,
) -> Result<Vec<String>, PageError> {
    let images = FileFilter::new("Images", &["png", "jpg", "jpeg"]);
    Ok(match request {
        Request::None => Vec::new(),
        Request::OpenFile => names_of(
            dialog(cancel, sink)
                .filter(images)
                .title("Open file")
                .open_file()?,
        ),
        Request::OpenFiles => names_of(
            dialog(cancel, sink)
                .filter(images)
                .title("Open files")
                .open_files()?,
        ),
        Request::SaveAs => names_of(
            dialog(cancel, sink)
                .title("Save as")
                .save_file(Some("frust-playground.txt"))?,
        ),
        Request::PickFolder => match dialog(cancel, sink).title("Pick folder").pick_folder()? {
            None => Vec::new(),
            Some(grant) => {
                let name = grant
                    .local_path()
                    .and_then(|path| path.file_name())
                    .map_or_else(
                        || "(folder)".to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                let listed = grant.list(&ListOptions::new().cancel(cancel.clone()));
                grant.release();
                let count = listed?.len();
                vec![format!("Folder: {name}"), format!("{count} file(s) inside")]
            }
        },
        Request::ExportFile => {
            let source = write_export_source()?;
            let outcome = dialog(cancel, sink)
                .title("Export file")
                .export_file(&source, Some(EXPORT_NAME));
            let _ = std::fs::remove_file(&source);
            match outcome? {
                ExportOutcome::Saved => vec![format!("Exported {EXPORT_NAME}")],
                _ => Vec::new(),
            }
        }
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
    /// The running call's token; Cancel fires it, each button press replaces it.
    token: Arc<Mutex<CancelToken>>,
    /// Written by the progress callback on a background thread; read only by the task future.
    progress_cell: ProgressCell,
    /// The latest progress, copied from the cell by the task future.
    progress: RwSignal<Option<FileProgress>>,
}

impl Component for DesktopPage {
    type State = DesktopPageState;

    fn init(&self) -> DesktopPageState {
        let request = Arc::new(Mutex::new(Request::None));
        let cell = request.clone();
        let token = Arc::new(Mutex::new(CancelToken::new()));
        let token_cell = token.clone();
        let progress_cell: ProgressCell = Arc::new(Mutex::new(None));
        let sink = progress_cell.clone();
        let progress: RwSignal<Option<FileProgress>> = RwSignal::new(None);
        let picked = use_task(move || {
            let request = *cell.lock().expect("request cell poisoned");
            let cancel = token_cell.lock().expect("token cell poisoned").clone();
            let sink = sink.clone();
            async move {
                let poll = sink.clone();
                let mut handle = spawn_blocking(move || run_dialog(request, &cancel, &sink));
                // The callback only writes the cell; copy it into the signal on the UI side
                // and stop once the page is gone.
                while !handle.is_finished() {
                    let latest = *poll.lock().expect("progress cell poisoned");
                    let Some(current) = progress.try_get_untracked() else {
                        break;
                    };
                    if latest != current {
                        progress.set(latest);
                    }
                    let _ = spawn_blocking(move || std::thread::sleep(PROGRESS_POLL)).await;
                }
                match (&mut handle).await {
                    Ok(result) => result,
                    Err(join_err) => Err(PageError::Other(join_err.to_string())),
                }
            }
        });
        DesktopPageState {
            picked,
            request,
            token,
            progress_cell,
            progress,
        }
    }

    fn build(&self, state: &mut DesktopPageState) -> impl View<DesktopPageState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let value = state.picked.signal().get();
        let busy = matches!(value, AsyncValue::Loading(_));
        let progress = state.progress.get();
        let (status, names) = status_lines(
            &value,
            loading_text(progress.map(|p| (p.done, p.items, p.fraction))),
        );
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
                        .child(dialog_button(state, "Open file", Request::OpenFile, busy))
                        .child(SizedBox(Some(8.0), None))
                        .child(dialog_button(state, "Open files", Request::OpenFiles, busy))
                        .child(SizedBox(Some(8.0), None))
                        .child(dialog_button(
                            state,
                            "Save as\u{2026}",
                            Request::SaveAs,
                            busy,
                        )),
                )
                .child(SizedBox(None, Some(8.0)))
                .child(
                    row()
                        .child(dialog_button(
                            state,
                            "Pick folder",
                            Request::PickFolder,
                            busy,
                        ))
                        .child(SizedBox(Some(8.0), None))
                        .child(dialog_button(
                            state,
                            "Export file",
                            Request::ExportFile,
                            busy,
                        ))
                        .child(SizedBox(Some(8.0), None))
                        .child(cancel_button(state, busy)),
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
    busy: bool,
) -> impl View<DesktopPageState> {
    let picked = state.picked.clone();
    let cell = state.request.clone();
    let (token, progress_cell, progress) = (
        state.token.clone(),
        state.progress_cell.clone(),
        state.progress,
    );
    button(label, move |_state: &mut DesktopPageState| {
        // A fresh token and a cleared progress cell for every press.
        *token.lock().expect("token cell poisoned") = CancelToken::new();
        *progress_cell.lock().expect("progress cell poisoned") = None;
        progress.set(None);
        *cell.lock().expect("request cell poisoned") = request;
        picked.restart();
    })
    .style(ButtonStyle::Secondary)
    .disabled(busy)
    .small()
}

/// Fires the running call's token; enabled only while a call runs.
fn cancel_button(state: &DesktopPageState, busy: bool) -> impl View<DesktopPageState> {
    let token = state.token.clone();
    button("Cancel", move |_state: &mut DesktopPageState| {
        let current = token.lock().expect("token cell poisoned").clone();
        current.cancel();
    })
    .style(ButtonStyle::Secondary)
    .disabled(!busy)
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
/// `loading` is the line shown while a call runs.
fn status_lines(value: &AsyncValue<Vec<String>>, loading: String) -> (String, Vec<String>) {
    match value {
        AsyncValue::Idle => ("Nothing picked yet.".to_string(), Vec::new()),
        AsyncValue::Loading(_) => (loading, Vec::new()),
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
        let sink: ProgressCell = Arc::new(Mutex::new(None));
        assert_eq!(
            run_dialog(Request::None, &CancelToken::new(), &sink),
            Ok(Vec::new())
        );
    }

    #[test]
    fn a_cancelled_token_fails_every_dialog_before_it_opens() {
        let token = CancelToken::new();
        token.cancel();
        let sink: ProgressCell = Arc::new(Mutex::new(None));
        for request in [
            Request::OpenFile,
            Request::OpenFiles,
            Request::SaveAs,
            Request::PickFolder,
            Request::ExportFile,
        ] {
            assert_eq!(
                run_dialog(request, &token, &sink),
                Err(PageError::Cancelled),
                "{request:?}"
            );
        }
    }

    #[test]
    fn cancelled_reads_cancelled() {
        assert_eq!(
            PageError::from(FileDialogError::Cancelled).to_string(),
            "Cancelled"
        );
    }

    #[test]
    fn status_waits_until_the_first_report() {
        assert_eq!(loading_text(None), "Waiting for the dialog\u{2026}");
    }

    #[test]
    fn status_shows_count_and_percent() {
        assert_eq!(loading_text(Some((1, 4, 0.25))), "Loading 1 of 4 (25%)");
        assert_eq!(loading_text(Some((4, 4, 1.0))), "Loading 4 of 4 (100%)");
    }

    #[test]
    fn idle_state_reads_nothing_picked() {
        let (status, names) = status_lines(&AsyncValue::Idle, "x".to_string());
        assert_eq!(status, "Nothing picked yet.");
        assert!(names.is_empty());
    }
}
