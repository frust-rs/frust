//! Photos section: the `frust-permissions` demo surface.
//!
//! A Permissions panel lists Photos (read/write), Photos (add-only), Camera,
//! Microphone and Media location. Each row shows the last status read for it
//! and a **Request** button; **Refresh** re-reads every status and **Open
//! settings** opens the app's system settings page.
//!
//! # Threading and reactivity
//!
//! `request` blocks until the OS answers, so a Request press stores its
//! target in a shared cell and restarts a [`use_task`] whose fetch runs the
//! call inside [`frust::spawn_blocking`] (the same request-cell shape as
//! `desktop_services`). Once the OS has answered, the same background step
//! reads the new status, so the row updates from the task result on the UI
//! thread. `build` never calls the plugin: statuses are read only from the
//! Refresh press and after a request finishes. Plugin errors
//! (`FeatureDisabled`, `MissingUsageDescription`, `Unsupported`, ...) render as
//! their `Display` text in the row.
//!
//! A Picker panel below it drives `frust-photo-library`: a filter selector
//! (Images / Videos / Both), a limit selector (1 / 5 / unlimited) and a
//! **Pick** button. The pick blocks until the user chooses, so it runs in
//! [`frust::spawn_blocking`] behind a [`use_task`]; the same background step
//! reads each item's size and, for the first image, its bytes (also off the UI
//! thread) and decodes them with [`frust::decode_image_async`] for the
//! preview. **Share picked** passes the picked files to
//! `Share::files_anchored` with a fixed anchor rectangle, and **Clear cache**
//! calls `PhotoLibrary::clear_pick_cache`. Only the PNG and JPEG decoders are
//! compiled in, so a HEIC, WebP or GIF preview shows a decode error.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use frust::authoring::Rect;
use frust::{
    AsyncValue, ButtonStyle, Component, EdgeInsets, Get, Image, ImageFit, ImageSource, Padding,
    RwSignal, Set, SizedBox, Theme, UseTask, View, button, column, component, decode_image_async,
    row, spawn_blocking, text, use_context, use_task,
};
use frust_permissions::{
    Permission, PermissionError, PermissionStatus, PhotoAccess, open_settings, request, status,
};
use frust_photo_library::{MediaFilter, MediaKind, PhotoLibrary, PickOptions, PickedItem};

use frust_share::{Share, ShareAnchor};

use crate::PlaygroundState;

/// One row per permission: its label and the permission it drives.
const ROWS: [(&str, Permission); 5] = [
    (
        "Photos (read/write)",
        Permission::Photos(PhotoAccess::ReadWrite),
    ),
    (
        "Photos (add-only)",
        Permission::Photos(PhotoAccess::AddOnly),
    ),
    ("Camera", Permission::Camera),
    ("Microphone", Permission::Microphone),
    ("Media location", Permission::MediaLocation),
];

/// Where the Share picked button sits on the page, in logical points. The
/// widget tier exposes no laid-out-rect seam, so this is the page's known
/// button position rather than a measured rect; it only positions an iOS
/// popover (or the macOS picker) and is ignored on every other platform.
const SHARE_ANCHOR_RECT: Rect = Rect::new(16.0, 720.0, 196.0, 752.0);

/// The filter selector's choices.
const FILTERS: [(&str, MediaFilter); 3] = [
    ("Images", MediaFilter::Images),
    ("Videos", MediaFilter::Videos),
    ("Both", MediaFilter::ImagesAndVideos),
];

/// The limit selector's choices; `0` is the platform maximum.
const LIMITS: [(&str, u32); 3] = [("1", 1), ("5", 5), ("Unlimited", 0)];

/// Shown for a row whose status has not been read yet.
const UNREAD: &str = "not read yet";

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(PhotosPage)
}

struct PhotosPage;

impl Component for PhotosPage {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Photos").size(13.0).color(accent))
                .child(
                    text(
                        "Runtime permissions through frust-permissions. Request asks the \
                         OS (off the UI thread); Refresh re-reads every status without \
                         prompting; Open settings jumps to the app's system settings.",
                    )
                    .size(11.0)
                    .color(muted),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(component(PermissionsPanel))
                .child(SizedBox(None, Some(16.0)))
                .child(component(PickerPanel)),
        )
    }
}

/// The task's error: only a join failure, as display text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JoinFailure(String);

impl fmt::Display for JoinFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for JoinFailure {}

/// A status, or the plugin error's `Display` text, as one row line.
fn describe(result: Result<PermissionStatus, PermissionError>) -> String {
    match result {
        Ok(status) => format!("{status:?}"),
        Err(err) => format!("error: {err}"),
    }
}

/// Runs the blocking request for row `index`, then reads its new status
/// (call inside `spawn_blocking`). `None` is the mount-time or Refresh-time
/// fetch: nothing asked.
fn run_request(index: Option<usize>) -> Option<(usize, String)> {
    let index = index?;
    let permission = ROWS[index].1;
    Some((
        index,
        match request(permission) {
            Ok(_) => describe(status(permission)),
            Err(err) => describe(Err(err)),
        },
    ))
}

struct PermissionsPanel;

struct PermissionsPanelState {
    /// Last status read per row, written by Refresh presses only.
    statuses: RwSignal<Vec<String>>,
    /// Row index of the pending request; `None` resolves to nothing.
    target: Arc<Mutex<Option<usize>>>,
    /// The latest request outcome; a Refresh press restarts it to clear it.
    answered: UseTask<Option<(usize, String)>>,
}

impl Component for PermissionsPanel {
    type State = PermissionsPanelState;

    fn init(&self) -> PermissionsPanelState {
        let target = Arc::new(Mutex::new(None));
        let cell = target.clone();
        let answered = use_task(move || {
            let index = cell.lock().expect("target cell poisoned").take();
            async move {
                spawn_blocking(move || run_request(index))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))
            }
        });
        PermissionsPanelState {
            statuses: RwSignal::new(vec![UNREAD.to_string(); ROWS.len()]),
            target,
            answered,
        }
    }

    fn build(&self, state: &mut PermissionsPanelState) -> impl View<PermissionsPanelState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let muted = theme.scheme().on_surface_variant;

        let statuses = state.statuses.get();
        let answered = state.answered.signal().get();
        let (answer, banner) = match &answered {
            AsyncValue::Loading(_) => (None, "Waiting for the OS\u{2026}".to_string()),
            AsyncValue::Ready(Some((index, line))) => (
                Some((*index, line.clone())),
                format!("{}: {line}", ROWS[*index].0),
            ),
            AsyncValue::Error(err) => (None, err.to_string()),
            _ => (None, "No request made yet.".to_string()),
        };

        let mut list = column();
        for (index, (label, _)) in ROWS.iter().enumerate() {
            let line = match &answer {
                Some((answered_index, line)) if *answered_index == index => line.clone(),
                _ => statuses[index].clone(),
            };
            list = list.child(
                row()
                    .child(text(*label).size(12.0))
                    .child(SizedBox(Some(12.0), None))
                    .child(text(line).size(12.0).color(muted))
                    .child(SizedBox(Some(12.0), None))
                    .child(request_button(state, index)),
            );
        }

        column()
            .child(list)
            .child(SizedBox(None, Some(8.0)))
            .child(text(banner).size(12.0).color(muted))
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(refresh_button(state))
                    .child(SizedBox(Some(8.0), None))
                    .child(
                        button("Open settings", |_state: &mut PermissionsPanelState| {
                            // Fire-and-forget; an unsupported platform has no settings page.
                            let _ = open_settings();
                        })
                        .style(ButtonStyle::Secondary)
                        .small(),
                    ),
            )
    }
}

fn request_button(state: &PermissionsPanelState, index: usize) -> impl View<PermissionsPanelState> {
    let answered = state.answered.clone();
    let target = state.target.clone();
    button("Request", move |_state: &mut PermissionsPanelState| {
        *target.lock().expect("target cell poisoned") = Some(index);
        answered.restart();
    })
    .style(ButtonStyle::Secondary)
    .small()
}

fn refresh_button(state: &PermissionsPanelState) -> impl View<PermissionsPanelState> {
    let answered = state.answered.clone();
    let statuses = state.statuses;
    button("Refresh", move |_state: &mut PermissionsPanelState| {
        statuses.set(ROWS.iter().map(|(_, p)| describe(status(*p))).collect());
        // Drop the last request's outcome so the rows show the fresh reads.
        answered.restart();
    })
    .style(ButtonStyle::Secondary)
    .small()
}

/// What the Pick / Clear cache buttons ask the background step to do.
#[derive(Debug, Clone, Copy)]
enum PickerAction {
    Pick(PickOptions),
    ClearCache,
}

/// One picked item as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemRow {
    name: String,
    kind: &'static str,
    mime: String,
    bytes: Option<u64>,
    copy: bool,
}

impl ItemRow {
    fn new(item: &PickedItem, bytes: Option<u64>) -> Self {
        Self {
            name: item
                .file_name
                .clone()
                .or_else(|| {
                    item.path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "(unnamed)".to_string()),
            kind: match item.kind {
                MediaKind::Image => "image",
                MediaKind::Video => "video",
                _ => "other",
            },
            mime: item.mime.clone().unwrap_or_else(|| "unknown mime".into()),
            bytes,
            copy: item.is_copy,
        }
    }

    fn line(&self) -> String {
        let size = self
            .bytes
            .map_or_else(|| "size unknown".to_string(), |b| format!("{b} bytes"));
        let origin = if self.copy { "copy" } else { "original" };
        format!(
            "{} | {} | {} | {size} | {origin}",
            self.name, self.kind, self.mime
        )
    }
}

/// The result of one Pick or Clear cache press.
#[derive(Clone)]
struct PickerOutcome {
    message: String,
    rows: Vec<ItemRow>,
    paths: Vec<PathBuf>,
    /// The first image's decode result, `None` when no image was picked.
    preview: Option<Result<ImageSource, String>>,
}

impl PickerOutcome {
    fn message(message: String) -> Self {
        Self {
            message,
            rows: Vec::new(),
            paths: Vec::new(),
            preview: None,
        }
    }
}

/// Runs one action (call inside `spawn_blocking`): the blocking pick or cache
/// clear, then each item's size. `None` is the mount-time fetch: nothing asked.
fn run_picker_action(action: Option<PickerAction>) -> Option<(PickerOutcome, Option<PathBuf>)> {
    match action? {
        PickerAction::ClearCache => Some((
            PickerOutcome::message(match PhotoLibrary::clear_pick_cache() {
                Ok(()) => "Pick cache cleared.".to_string(),
                Err(err) => format!("error: {err}"),
            }),
            None,
        )),
        PickerAction::Pick(options) => Some(match PhotoLibrary::pick(&options) {
            Err(err) => (PickerOutcome::message(format!("error: {err}")), None),
            Ok(items) if items.is_empty() => (
                PickerOutcome::message("Cancelled: nothing picked.".into()),
                None,
            ),
            Ok(items) => {
                let rows = items
                    .iter()
                    .map(|item| {
                        let bytes = std::fs::metadata(&item.path).ok().map(|m| m.len());
                        ItemRow::new(item, bytes)
                    })
                    .collect::<Vec<_>>();
                let first_image = items
                    .iter()
                    .find(|item| item.kind == MediaKind::Image)
                    .map(|item| item.path.clone());
                let outcome = PickerOutcome {
                    message: format!("Picked {} item(s).", items.len()),
                    rows,
                    paths: items.into_iter().map(|item| item.path).collect(),
                    preview: None,
                };
                (outcome, first_image)
            }
        }),
    }
}

struct PickerPanel;

struct PickerPanelState {
    /// Index into [`FILTERS`], written by the filter buttons only.
    filter: RwSignal<usize>,
    /// Index into [`LIMITS`], written by the limit buttons only.
    limit: RwSignal<usize>,
    /// The pending action; `None` resolves to nothing.
    action: Arc<Mutex<Option<PickerAction>>>,
    /// The latest outcome; a press restarts it.
    outcome: UseTask<Option<PickerOutcome>>,
    /// The last Share picked result line.
    shared: RwSignal<String>,
}

impl Component for PickerPanel {
    type State = PickerPanelState;

    fn init(&self) -> PickerPanelState {
        let action = Arc::new(Mutex::new(None));
        let cell = action.clone();
        let outcome = use_task(move || {
            let requested = cell.lock().expect("action cell poisoned").take();
            async move {
                let step = spawn_blocking(move || run_picker_action(requested))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))?;
                let Some((mut outcome, first_image)) = step else {
                    return Ok::<Option<PickerOutcome>, JoinFailure>(None);
                };
                if let Some(path) = first_image {
                    // Read off the UI thread, then decode off it too.
                    let bytes = spawn_blocking(move || std::fs::read(path))
                        .await
                        .map_err(|err| JoinFailure(err.to_string()))?;
                    outcome.preview = Some(match bytes {
                        Ok(bytes) => decode_image_async(bytes)
                            .await
                            .map_err(|err| format!("preview decode error: {err}")),
                        Err(err) => Err(format!("preview read error: {err}")),
                    });
                }
                Ok(Some(outcome))
            }
        });
        PickerPanelState {
            filter: RwSignal::new(2),
            limit: RwSignal::new(0),
            action,
            outcome,
            shared: RwSignal::new(String::new()),
        }
    }

    fn build(&self, state: &mut PickerPanelState) -> impl View<PickerPanelState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let selected_filter = state.filter.get();
        let selected_limit = state.limit.get();
        let shared = state.shared.get();
        let outcome = state.outcome.signal().get();

        let mut filters = row();
        for (index, (label, _)) in FILTERS.iter().enumerate() {
            let signal = state.filter;
            filters = filters.child(
                button(*label, move |_state: &mut PickerPanelState| {
                    signal.set(index);
                })
                .style(selector_style(index == selected_filter))
                .small(),
            );
            filters = filters.child(SizedBox(Some(8.0), None));
        }
        let mut limits = row();
        for (index, (label, _)) in LIMITS.iter().enumerate() {
            let signal = state.limit;
            limits = limits.child(
                button(*label, move |_state: &mut PickerPanelState| {
                    signal.set(index);
                })
                .style(selector_style(index == selected_limit))
                .small(),
            );
            limits = limits.child(SizedBox(Some(8.0), None));
        }

        let mut paths: Vec<PathBuf> = Vec::new();
        let mut body = column();
        match &outcome {
            AsyncValue::Loading(_) => {
                body = body.child(
                    text("Waiting for the picker\u{2026}")
                        .size(12.0)
                        .color(muted),
                );
            }
            AsyncValue::Ready(Some(done)) => {
                body = body.child(text(done.message.clone()).size(12.0).color(muted));
                for item in &done.rows {
                    body = body.child(text(item.line()).size(11.0));
                }
                match &done.preview {
                    Some(Ok(source)) => {
                        body = body.child(SizedBox(None, Some(8.0))).child(
                            SizedBox(Some(160.0), Some(160.0))
                                .child(Image(source.clone()).fit(ImageFit::Contain)),
                        );
                    }
                    Some(Err(message)) => {
                        body = body.child(text(message.clone()).size(11.0).color(muted));
                    }
                    None => {}
                }
                paths = done.paths.clone();
            }
            AsyncValue::Error(err) => {
                body = body.child(text(err.to_string()).size(12.0).color(muted));
            }
            _ => {
                body = body.child(text("Nothing picked yet.").size(12.0).color(muted));
            }
        }

        column()
            .child(text("Picker").size(13.0).color(accent))
            .child(
                text(
                    "Pick runs the system photo picker off the UI thread. The first \
                     image is previewed (PNG and JPEG decode; HEIC, WebP and GIF show \
                     a decode error). Share picked sends the files; Clear cache \
                     removes the copies made on mobile.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(text("Filter").size(12.0))
            .child(filters)
            .child(SizedBox(None, Some(4.0)))
            .child(text("Limit").size(12.0))
            .child(limits)
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(pick_button(state))
                    .child(SizedBox(Some(8.0), None))
                    .child(share_button(state, paths))
                    .child(SizedBox(Some(8.0), None))
                    .child(clear_button(state)),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(body)
            .child(text(shared).size(11.0).color(muted))
    }
}

fn selector_style(selected: bool) -> ButtonStyle {
    if selected {
        ButtonStyle::Primary
    } else {
        ButtonStyle::Secondary
    }
}

fn pick_button(state: &PickerPanelState) -> impl View<PickerPanelState> {
    let (filter, limit) = (state.filter, state.limit);
    let action = state.action.clone();
    let outcome = state.outcome.clone();
    button("Pick", move |_state: &mut PickerPanelState| {
        let options = PickOptions::new()
            .filter(FILTERS[filter.get()].1)
            .limit(LIMITS[limit.get()].1);
        *action.lock().expect("action cell poisoned") = Some(PickerAction::Pick(options));
        outcome.restart();
    })
    .small()
}

fn clear_button(state: &PickerPanelState) -> impl View<PickerPanelState> {
    let action = state.action.clone();
    let outcome = state.outcome.clone();
    button("Clear cache", move |_state: &mut PickerPanelState| {
        *action.lock().expect("action cell poisoned") = Some(PickerAction::ClearCache);
        outcome.restart();
    })
    .style(ButtonStyle::Secondary)
    .small()
}

fn share_button(state: &PickerPanelState, paths: Vec<PathBuf>) -> impl View<PickerPanelState> {
    let shared = state.shared;
    button("Share picked", move |_state: &mut PickerPanelState| {
        let refs: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
        let anchor = ShareAnchor {
            rect: SHARE_ANCHOR_RECT,
        };
        shared.set(format!(
            "Share::files_anchored -> {:?}",
            Share::files_anchored(&refs, None, Some(&anchor))
        ));
    })
    .style(ButtonStyle::Secondary)
    .small()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_target_resolves_to_nothing() {
        assert_eq!(run_request(None), None);
    }

    #[test]
    fn rows_cover_every_permission() {
        assert_eq!(ROWS.len(), 5);
        assert!(ROWS.iter().any(|(_, p)| *p == Permission::MediaLocation));
    }

    #[test]
    fn errors_render_as_display_text() {
        assert_eq!(
            describe(Err(PermissionError::Unsupported)),
            "error: runtime permissions are not supported on this platform"
        );
        assert_eq!(describe(Ok(PermissionStatus::Granted)), "Granted");
    }

    #[test]
    fn no_action_resolves_to_nothing() {
        assert!(run_picker_action(None).is_none());
    }

    #[test]
    fn selectors_cover_the_documented_choices() {
        assert_eq!(FILTERS.len(), 3);
        assert_eq!(LIMITS.map(|(_, n)| n), [1, 5, 0]);
    }

    #[test]
    fn item_line_lists_name_kind_mime_size_and_origin() {
        let row = ItemRow {
            name: "a.png".into(),
            kind: "image",
            mime: "image/png".into(),
            bytes: Some(12),
            copy: true,
        };
        assert_eq!(row.line(), "a.png | image | image/png | 12 bytes | copy");
    }
}
