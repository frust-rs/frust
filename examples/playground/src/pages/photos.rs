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
//! preview. While a pick runs the status line reads `Waiting for the picker...`
//! until the first progress report, then `Loading {done} of {items} ({pct}%)`;
//! Pick and Clear cache are disabled and a **Cancel** button calls the pick's
//! own [`CancelToken`] (a fresh one per press). The progress callback runs on a
//! background thread, so it only writes a shared cell; the task future polls
//! that cell and copies changes into a signal. **Share picked** passes the
//! picked files to `Share::files_anchored` with a fixed anchor rectangle, and
//! **Clear cache** calls `PhotoLibrary::clear_pick_cache`. Only the PNG and JPEG
//! decoders are compiled in, so a HEIC, WebP or GIF preview shows a decode
//! error.
//!
//! A Browse panel below the Picker lists the library through the same plugin's `library`
//! feature: an access line (the Photos read/write status, read on **Refresh**), a
//! **Manage selection** banner while access is Limited, an album selector, and a keyed lazy
//! `ListView` of rows of four thumbnails. Assets load in pages of [`PAGE_SIZE`], the next page
//! only when **Load more** is pressed, with the loaded and total counts shown. Every
//! plugin call runs in [`frust::spawn_blocking`] behind a [`use_task`]; each thumbnail cell is
//! its own component with a task keyed by the asset id, requesting a thumbnail at
//! [`THUMB_SCALE`] times the cell size. Browsing is read-only and never takes the pick guard,
//! so it works while a pick is open; Manage selection presents system UI and does take it.
//! When a picked item carries an asset id (iOS), the Picker preview tries the native thumbnail
//! first and decodes the copied file only when that fails. A HEIC pick therefore previews when
//! the library grants access, and otherwise shows the file's decode error.
//!
//! An Originals panel below Browse acts on one asset: the thumbnail tapped in the Browse grid
//! (it insets while selected) or, without a selection, the Picker's most recent asset id.
//! Version (Original / Current) and Network (allowed / off) toggles feed
//! `PhotoLibrary::export_original`, which runs in [`frust::spawn_blocking`] behind a
//! [`use_task`]. Its progress callback writes only a shared cell; the task future polls the
//! cell and copies it into a signal (the same shape as the Picker), driving a status line and a
//! bar, and **Cancel** calls the export's own [`CancelToken`]. The result line shows the file
//! name, mime, byte size and the first 12 bytes in hex with a printable-text rendering, so
//! `ftypheic` (HEIC) or `II*` (DNG) is visible. **Save copy back** files the exported copy
//! through `PhotoLibrary::save` into the album `Frust Playground`; **Save (no album)** is the
//! add-only variant. Neither export nor save takes the pick guard.

use std::collections::HashSet;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use frust::authoring::Rect;
use frust::{
    AsyncValue, ButtonStyle, ChildKey, Component, EdgeInsets, GestureDetector, Get, GetUntracked,
    Image, ImageFit, ImageSource, ListView, Padding, RwSignal, ScrollbarStyle, Set, SizedBox,
    Theme, UseTask, View, button, colored_box, column, component, decode_image_async, either, row,
    spawn_blocking, text, use_context, use_task,
};
use frust_file_dialogs::{
    CancelToken as FolderCancel, FileDialog, FileDialogError, FileProgress, FolderGrant,
};
use frust_permissions::{
    Permission, PermissionError, PermissionStatus, PhotoAccess, open_settings, request, status,
};
use frust_photo_library::folder::FolderEntry;
use frust_photo_library::{
    Album, AlbumId, Asset, AssetId, AssetPage, AssetQuery, AssetVersion, CancelToken,
    ExportOptions, ExportedOriginal, FolderMedia, FolderPage, FolderScan, MediaFilter, MediaKind,
    PhotoError, PhotoLibrary, PickOptions, PickProgress, PickedItem, SaveRequest, ThumbnailMode,
    ThumbnailRequest,
};
use frust_shared_preferences::SharedPreferences;

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

/// The asset ids the Browse and Picker panels hand to the Originals panel.
#[derive(Clone, Copy)]
struct SharedAssets {
    /// The asset tapped in the Browse grid.
    selected: RwSignal<Option<AssetId>>,
    /// The first asset id of the Picker's most recent pick that carried one.
    picked: RwSignal<Option<AssetId>>,
}

impl Component for PhotosPage {
    type State = SharedAssets;

    fn init(&self) -> SharedAssets {
        SharedAssets {
            selected: RwSignal::new(None),
            picked: RwSignal::new(None),
        }
    }

    fn build(&self, state: &mut SharedAssets) -> impl View<SharedAssets> {
        let shared = *state;
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
                .child(component(PickerPanel {
                    picked: shared.picked,
                }))
                .child(SizedBox(None, Some(16.0)))
                .child(component(BrowsePanel {
                    selected: shared.selected,
                }))
                .child(SizedBox(None, Some(16.0)))
                .child(component(OriginalsPanel {
                    selected: shared.selected,
                    picked: shared.picked,
                }))
                .child(SizedBox(None, Some(16.0)))
                .child(component(FoldersPanel)),
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
#[derive(Debug, Clone)]
enum PickerAction {
    Pick(PickOptions),
    ClearCache,
}

/// How often the task polls the progress cell while a pick runs.
const PROGRESS_POLL: Duration = Duration::from_millis(100);

/// The status line while a pick runs: waiting until the first report, then the
/// loaded count and percentage. `progress` is `(done, items, fraction)`.
fn loading_text(progress: Option<(usize, usize, f64)>) -> String {
    match progress {
        None => "Waiting for the picker\u{2026}".to_string(),
        Some((done, items, fraction)) => {
            let pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
            format!("Loading {done} of {items} ({pct}%)")
        }
    }
}

/// A failed pick as one status line: a cancel is plain `Cancelled`, anything
/// else (including `Busy`) is its `Display` text.
fn pick_error_text(err: &PhotoError) -> String {
    match err {
        PhotoError::Cancelled => "Cancelled".to_string(),
        other => format!("error: {other}"),
    }
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
    /// The first picked item's library asset id, when one carried it.
    asset_id: Option<AssetId>,
    /// The first image's decode result, `None` when no image was picked.
    preview: Option<Result<ImageSource, String>>,
}

impl PickerOutcome {
    fn message(message: String) -> Self {
        Self {
            message,
            rows: Vec::new(),
            paths: Vec::new(),
            asset_id: None,
            preview: None,
        }
    }
}

/// Where the Picker preview comes from.
#[derive(Debug, Clone)]
enum PreviewSource {
    /// The item has a library asset id: try the native thumbnail first, and fall back to
    /// decoding the picked file at `path` when it fails.
    Asset { id: AssetId, path: PathBuf },
    /// No asset id (Android, desktop): decode the picked file.
    File(PathBuf),
}

/// The preview source for a picked image: its asset id when it has one.
fn preview_source(item: &PickedItem) -> PreviewSource {
    match &item.asset_id {
        Some(id) => PreviewSource::Asset {
            id: id.clone(),
            path: item.path.clone(),
        },
        None => PreviewSource::File(item.path.clone()),
    }
}

/// Whether an asset-backed preview must decode the picked file instead. Any thumbnail failure
/// does (denied access, a missing asset, a format the thumbnailer cannot read): the file decode
/// is the baseline outcome and the thumbnail only adds capability, so it never removes that
/// outcome.
fn falls_back_to_file(thumbnail: &Result<ImageSource, String>) -> bool {
    thumbnail.is_err()
}

/// The preview decoded from the picked file: read off the UI thread, then decoded off it too.
async fn file_preview(path: PathBuf) -> Result<Result<ImageSource, String>, JoinFailure> {
    let bytes = spawn_blocking(move || std::fs::read(path))
        .await
        .map_err(|err| JoinFailure(err.to_string()))?;
    Ok(match bytes {
        Ok(bytes) => decode_image_async(bytes)
            .await
            .map_err(|err| format!("preview decode error: {err}")),
        Err(err) => Err(format!("preview read error: {err}")),
    })
}

/// The preview's thumbnail box edge in pixels: the 160 point box at [`THUMB_SCALE`].
const PREVIEW_EDGE_PX: u32 = 160 * THUMB_SCALE;

/// A native thumbnail as an image source (call inside `spawn_blocking`).
fn load_thumbnail(id: &AssetId, edge_px: u32, mode: ThumbnailMode) -> Result<ImageSource, String> {
    PhotoLibrary::thumbnail(id, &ThumbnailRequest::new(edge_px, edge_px, mode))
        .map(|thumb| ImageSource::from_rgba8(thumb.rgba, thumb.width, thumb.height))
        .map_err(|err| err.to_string())
}

/// Runs one action (call inside `spawn_blocking`): the blocking pick or cache
/// clear, then each item's size. `None` is the mount-time fetch: nothing asked.
fn run_picker_action(
    action: Option<PickerAction>,
) -> Option<(PickerOutcome, Option<PreviewSource>)> {
    match action? {
        PickerAction::ClearCache => Some((
            PickerOutcome::message(match PhotoLibrary::clear_pick_cache() {
                Ok(()) => "Pick cache cleared.".to_string(),
                Err(err) => format!("error: {err}"),
            }),
            None,
        )),
        PickerAction::Pick(options) => Some(match PhotoLibrary::pick(&options) {
            Err(err) => (PickerOutcome::message(pick_error_text(&err)), None),
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
                    .map(preview_source);
                let outcome = PickerOutcome {
                    message: format!("Picked {} item(s).", items.len()),
                    rows,
                    asset_id: items.iter().find_map(|item| item.asset_id.clone()),
                    paths: items.into_iter().map(|item| item.path).collect(),
                    preview: None,
                };
                (outcome, first_image)
            }
        }),
    }
}

struct PickerPanel {
    /// Receives the most recent pick's asset id for the Originals panel.
    picked: RwSignal<Option<AssetId>>,
}

struct PickerPanelState {
    /// Index into [`FILTERS`], written by the filter buttons only.
    filter: RwSignal<usize>,
    /// Index into [`LIMITS`], written by the limit buttons only.
    limit: RwSignal<usize>,
    /// The pending action; `None` resolves to nothing.
    action: Arc<Mutex<Option<PickerAction>>>,
    /// The latest outcome; a press restarts it.
    outcome: UseTask<Option<PickerOutcome>>,
    /// Written by the progress callback on a background thread; read only by
    /// the task future and the Pick handler.
    progress_cell: Arc<Mutex<Option<PickProgress>>>,
    /// The latest progress, copied from the cell by the task future.
    progress: RwSignal<Option<PickProgress>>,
    /// The running pick's token; Cancel takes it, each Pick press replaces it.
    token: Arc<Mutex<Option<CancelToken>>>,
    /// The last Share picked result line.
    shared: RwSignal<String>,
}

impl Component for PickerPanel {
    type State = PickerPanelState;

    fn init(&self) -> PickerPanelState {
        let action = Arc::new(Mutex::new(None));
        let cell = action.clone();
        let progress_cell: Arc<Mutex<Option<PickProgress>>> = Arc::new(Mutex::new(None));
        let progress = RwSignal::new(None);
        let poll_cell = progress_cell.clone();
        let picked = self.picked;
        let outcome = use_task(move || {
            let requested = cell.lock().expect("action cell poisoned").take();
            let poll_cell = poll_cell.clone();
            async move {
                let mut handle = spawn_blocking(move || run_picker_action(requested));
                // Poll the cell the callback thread writes. The panel can unmount while
                // this runs, so stop copying once the signal is gone.
                while !handle.is_finished() {
                    let latest = *poll_cell.lock().expect("progress cell poisoned");
                    let Some(current) = progress.try_get_untracked() else {
                        break;
                    };
                    if latest != current {
                        progress.set(latest);
                    }
                    let _ = spawn_blocking(move || std::thread::sleep(PROGRESS_POLL)).await;
                }
                let step = (&mut handle)
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))?;
                let Some((mut outcome, first_image)) = step else {
                    return Ok::<Option<PickerOutcome>, JoinFailure>(None);
                };
                if let Some(id) = &outcome.asset_id
                    && picked.try_get_untracked().is_some()
                {
                    picked.set(Some(id.clone()));
                }
                match first_image {
                    Some(PreviewSource::Asset { id, path }) => {
                        let thumb = spawn_blocking(move || {
                            load_thumbnail(&id, PREVIEW_EDGE_PX, ThumbnailMode::Fit)
                        })
                        .await
                        .map_err(|err| JoinFailure(err.to_string()))?;
                        outcome.preview = Some(if falls_back_to_file(&thumb) {
                            file_preview(path).await?
                        } else {
                            thumb
                        });
                    }
                    Some(PreviewSource::File(path)) => {
                        outcome.preview = Some(file_preview(path).await?);
                    }
                    None => {}
                }
                Ok(Some(outcome))
            }
        });
        PickerPanelState {
            filter: RwSignal::new(2),
            limit: RwSignal::new(0),
            action,
            outcome,
            progress_cell,
            progress,
            token: Arc::new(Mutex::new(None)),
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
        let busy = matches!(outcome, AsyncValue::Loading(_));
        let progress = state.progress.get();

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
                let line = loading_text(progress.map(|p| (p.done, p.items, p.fraction)));
                body = body.child(text(line).size(12.0).color(muted));
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
                     a decode error, unless an item with a library asset id previews \
                     through the native thumbnail first). Share picked sends the files; \
                     Clear cache removes the copies made on mobile.",
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
                    .child(pick_button(state, busy))
                    .child(SizedBox(Some(8.0), None))
                    .child(share_button(state, paths))
                    .child(SizedBox(Some(8.0), None))
                    .child(clear_button(state, busy))
                    .child(SizedBox(Some(8.0), None))
                    .child(cancel_button(state, busy)),
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

fn pick_button(state: &PickerPanelState, busy: bool) -> impl View<PickerPanelState> {
    let (filter, limit) = (state.filter, state.limit);
    let action = state.action.clone();
    let outcome = state.outcome.clone();
    let (progress_cell, progress, token) = (
        state.progress_cell.clone(),
        state.progress,
        state.token.clone(),
    );
    button("Pick", move |_state: &mut PickerPanelState| {
        // A fresh token and a cleared progress cell for every pick.
        let cancel = CancelToken::new();
        *token.lock().expect("token cell poisoned") = Some(cancel.clone());
        *progress_cell.lock().expect("progress cell poisoned") = None;
        progress.set(None);
        let sink = progress_cell.clone();
        let options = PickOptions::new()
            .filter(FILTERS[filter.get()].1)
            .limit(LIMITS[limit.get()].1)
            .on_progress(move |p| {
                *sink.lock().expect("progress cell poisoned") = Some(p);
            })
            .cancel(cancel);
        *action.lock().expect("action cell poisoned") = Some(PickerAction::Pick(options));
        outcome.restart();
    })
    .disabled(busy)
    .small()
}

fn cancel_button(state: &PickerPanelState, busy: bool) -> impl View<PickerPanelState> {
    let token = state.token.clone();
    button("Cancel", move |_state: &mut PickerPanelState| {
        let current = token.lock().expect("token cell poisoned").clone();
        if let Some(current) = current {
            current.cancel();
        }
    })
    .style(ButtonStyle::Secondary)
    .disabled(!busy)
    .small()
}

fn clear_button(state: &PickerPanelState, busy: bool) -> impl View<PickerPanelState> {
    let action = state.action.clone();
    let outcome = state.outcome.clone();
    let (progress_cell, progress) = (state.progress_cell.clone(), state.progress);
    button("Clear cache", move |_state: &mut PickerPanelState| {
        // The status line must not show the last pick's progress during the clear.
        *progress_cell.lock().expect("progress cell poisoned") = None;
        progress.set(None);
        *action.lock().expect("action cell poisoned") = Some(PickerAction::ClearCache);
        outcome.restart();
    })
    .style(ButtonStyle::Secondary)
    .disabled(busy)
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

/// Assets per page; the next page loads only from the Load more button.
const PAGE_SIZE: u32 = 60;

/// Thumbnail cells per list row.
const COLUMNS: usize = 4;

/// A thumbnail cell's edge in logical points.
const CELL: f64 = 76.0;

/// The gap between cells and between rows, in logical points.
const GAP: f64 = 4.0;

/// One list row's height: a cell plus the gap below it.
const ROW_EXTENT: f64 = CELL + GAP;

/// Thumbnails are requested at this multiple of the cell size, for dense screens.
const THUMB_SCALE: u32 = 3;

/// The thumbnail request edge for a cell, in pixels.
const CELL_EDGE_PX: u32 = CELL as u32 * THUMB_SCALE;

/// The bounded height the thumbnail list gets inside the page's scrolling column.
const LIST_VIEWPORT_H: f64 = 340.0;

/// Album buttons shown before the selector truncates.
const MAX_ALBUM_BUTTONS: usize = 12;

/// The asset ids loaded so far for the selected album, and the library's total for it.
#[derive(Clone, Default)]
struct Listing {
    ids: Arc<Vec<AssetId>>,
    total: u64,
}

/// List rows needed for `count` assets.
fn row_count(count: usize) -> usize {
    count.div_ceil(COLUMNS)
}

/// The asset ids of list row `row`.
fn row_ids(ids: &[AssetId], row: usize) -> &[AssetId] {
    let start = (row * COLUMNS).min(ids.len());
    let end = (start + COLUMNS).min(ids.len());
    &ids[start..end]
}

/// A row's identity: the ids of its assets, so a row keeps its thumbnail tasks across a Load
/// more and gets fresh ones when an album switch puts other assets in the same slot.
fn row_key(ids: &[AssetId], row: usize) -> ChildKey {
    let names: Vec<&str> = row_ids(ids, row).iter().map(AssetId::as_str).collect();
    ChildKey::new(names)
}

/// `existing` followed by the `page` ids it does not already hold.
fn merge_page(existing: &[AssetId], page: Vec<AssetId>) -> Vec<AssetId> {
    let mut seen: HashSet<AssetId> = existing.iter().cloned().collect();
    let mut merged = existing.to_vec();
    merged.extend(page.into_iter().filter(|id| seen.insert(id.clone())));
    merged
}

/// The ids of a page's assets.
fn page_ids(items: Vec<Asset>) -> Vec<AssetId> {
    items.into_iter().map(|asset| asset.id).collect()
}

/// Whether another page can exist: fewer assets loaded than the total.
fn has_more(loaded: usize, total: u64) -> bool {
    (loaded as u64) < total
}

/// The count line under the list.
fn count_line(loaded: usize, total: u64) -> String {
    format!("Showing {loaded} of {total} assets")
}

/// The query for the page starting at `offset`.
fn browse_query(album: &Option<AlbumId>, offset: u64) -> AssetQuery {
    let query = AssetQuery::new().limit(PAGE_SIZE).offset(offset);
    match album {
        Some(id) => query.album(id.clone()),
        None => query,
    }
}

/// The access line: the Photos read/write status as text.
fn access_line(result: Result<PermissionStatus, PermissionError>) -> String {
    format!("Photo access: {}", describe(result))
}

/// Reads the Photos read/write status: its access line and whether it is Limited.
fn read_access() -> (String, bool) {
    let result = status(Permission::Photos(PhotoAccess::ReadWrite));
    let limited = matches!(result, Ok(PermissionStatus::Limited));
    (access_line(result), limited)
}

/// An album button label: its title and, when reported, its count.
fn album_label(album: &Album) -> String {
    match album.count {
        Some(count) => format!("{} ({count})", album.title),
        None => album.title.clone(),
    }
}

/// What a Browse button asks the background step to do.
#[derive(Debug, Clone)]
enum BrowseAction {
    /// Re-list the albums and the first page of the selected album.
    Refresh(Option<AlbumId>),
    /// The first page of another album.
    Select(Option<AlbumId>),
    /// The page after the `offset` assets already loaded.
    More(Option<AlbumId>, u64),
    /// Present the limited-selection UI, then re-read access and re-list.
    Manage(Option<AlbumId>),
}

/// How a loaded page combines with the current listing.
enum PageUpdate {
    Replace(AssetPage),
    Append(AssetPage),
}

/// What one background step learned; each part is `None` when the step did not touch it.
#[derive(Default)]
struct BrowseStep {
    access: Option<(String, bool)>,
    albums: Option<Vec<Album>>,
    /// The album a replacing page was loaded for. Set only with that page, so a failed album
    /// switch never moves the selection away from the listing it still shows.
    album: Option<Option<AlbumId>>,
    page: Option<PageUpdate>,
    /// Empty on success, else the failure as `error: ...` text.
    message: String,
}

/// Loads one page into `step`, or records the failure.
fn load_page(album: &Option<AlbumId>, offset: u64, append: bool, step: &mut BrowseStep) {
    record_page(
        album,
        append,
        PhotoLibrary::assets(&browse_query(album, offset)),
        step,
    );
}

/// Records one page result into `step`. A replacing page commits `album` as the selection with
/// the listing; an appended page keeps the listing's album; a failure changes neither.
fn record_page(
    album: &Option<AlbumId>,
    append: bool,
    result: Result<AssetPage, PhotoError>,
    step: &mut BrowseStep,
) {
    match result {
        Ok(page) if append => step.page = Some(PageUpdate::Append(page)),
        Ok(page) => {
            step.page = Some(PageUpdate::Replace(page));
            step.album = Some(album.clone());
        }
        Err(err) => step.message = format!("error: {err}"),
    }
}

/// Albums, then the first page, into `step` (stops at the first failure).
fn load_albums_and_first_page(album: &Option<AlbumId>, step: &mut BrowseStep) {
    match PhotoLibrary::albums() {
        Ok(albums) => {
            step.albums = Some(albums);
            load_page(album, 0, false, step);
        }
        Err(err) => step.message = format!("error: {err}"),
    }
}

/// Runs one action (call inside `spawn_blocking`). `None` is the mount-time fetch.
fn run_browse(action: Option<BrowseAction>) -> Option<BrowseStep> {
    let mut step = BrowseStep::default();
    match action? {
        BrowseAction::Refresh(album) => load_albums_and_first_page(&album, &mut step),
        BrowseAction::Select(album) => load_page(&album, 0, false, &mut step),
        BrowseAction::More(album, offset) => load_page(&album, offset, true, &mut step),
        BrowseAction::Manage(album) => match PhotoLibrary::manage_limited_selection() {
            Ok(()) => {
                step.access = Some(read_access());
                load_albums_and_first_page(&album, &mut step);
            }
            Err(err) => step.message = format!("error: {err}"),
        },
    }
    Some(step)
}

/// The panel's signals, copied into the task future.
#[derive(Clone, Copy)]
struct BrowseSignals {
    access: RwSignal<String>,
    limited: RwSignal<bool>,
    albums: RwSignal<Vec<Album>>,
    album: RwSignal<Option<AlbumId>>,
    listing: RwSignal<Listing>,
    message: RwSignal<String>,
}

impl BrowseSignals {
    /// Copies a finished step into the signals. Called from the task future, which runs on the
    /// tokio runtime rather than the UI thread. Skipped once the panel is gone.
    fn apply(&self, step: BrowseStep) {
        if self.message.try_get_untracked().is_none() {
            return;
        }
        if let Some((line, limited)) = step.access {
            self.access.set(line);
            self.limited.set(limited);
        }
        if let Some(albums) = step.albums {
            self.albums.set(albums);
        }
        if let Some(album) = step.album {
            self.album.set(album);
        }
        match step.page {
            Some(PageUpdate::Replace(page)) => self.listing.set(Listing {
                ids: Arc::new(page_ids(page.items)),
                total: page.total,
            }),
            Some(PageUpdate::Append(page)) => {
                let current = self.listing.get_untracked();
                self.listing.set(Listing {
                    ids: Arc::new(merge_page(&current.ids, page_ids(page.items))),
                    total: page.total,
                });
            }
            None => {}
        }
        self.message.set(step.message);
    }
}

/// One thumbnail cell. Its task is keyed by the asset id: the cell is built for one asset and
/// requests that asset's thumbnail once, off the UI thread.
struct ThumbCell {
    id: AssetId,
    /// Set to this cell's asset on a tap; the cell insets itself while it is the selection.
    selected: RwSignal<Option<AssetId>>,
}

impl Component for ThumbCell {
    type State = UseTask<Result<ImageSource, String>>;

    fn init(&self) -> Self::State {
        let id = self.id.clone();
        use_task(move || {
            let id = id.clone();
            async move {
                spawn_blocking(move || load_thumbnail(&id, CELL_EDGE_PX, ThumbnailMode::Fill))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))
            }
        })
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let scheme = theme.scheme();
        let value = state.signal().get();
        let (source, failed) = match value {
            AsyncValue::Ready(Ok(source)) => (Some(source), false),
            AsyncValue::Ready(Err(_)) | AsyncValue::Error(_) => (None, true),
            _ => (None, false),
        };
        let (empty, error_fill) = (scheme.surface_container_highest, scheme.error_container);
        let is_selected = self.selected.get().as_ref() == Some(&self.id);
        let target = self.id.clone();
        let selected = self.selected;
        let content = move || thumb_content(source.clone(), failed, empty, error_fill);
        let inset_content = content.clone();
        GestureDetector(SizedBox(Some(CELL), Some(CELL)).child(either(
            is_selected,
            move || Padding(EdgeInsets::all(SELECTED_INSET), inset_content()),
            move || Padding(EdgeInsets::all(0.0), content()),
        )))
        .on_tap(move |_state: &mut Self::State| selected.set(Some(target.clone())))
    }
}

/// The inset around the selected cell's thumbnail, in logical points.
const SELECTED_INSET: f64 = 8.0;

/// A cell's picture: the thumbnail, or a flat placeholder (tinted when its load failed).
fn thumb_content(
    source: Option<ImageSource>,
    failed: bool,
    empty: frust::Color,
    error_fill: frust::Color,
) -> impl View<UseTask<Result<ImageSource, String>>> {
    either(
        source.is_some(),
        move || Image(source.clone().expect("checked above")).fit(ImageFit::Cover),
        move || {
            either(
                failed,
                move || colored_box().fill(error_fill).radius(4.0),
                move || colored_box().fill(empty).radius(4.0),
            )
        },
    )
}

struct BrowsePanel {
    /// Written by a tap on a thumbnail cell.
    selected: RwSignal<Option<AssetId>>,
}

struct BrowsePanelState {
    /// The asset tapped in the grid, shared with the Originals panel.
    selected: RwSignal<Option<AssetId>>,
    /// The access line, written by Refresh and after Manage selection.
    access: RwSignal<String>,
    /// Whether access is Limited, which shows the Manage selection banner.
    limited: RwSignal<bool>,
    /// The albums from the last Refresh.
    albums: RwSignal<Vec<Album>>,
    /// The selected album; `None` is the whole library.
    album: RwSignal<Option<AlbumId>>,
    /// The loaded assets and the total.
    listing: RwSignal<Listing>,
    /// The last failure, empty when none.
    message: RwSignal<String>,
    /// The pending action; `None` resolves to nothing.
    action: Arc<Mutex<Option<BrowseAction>>>,
    /// The running step; a press restarts it.
    work: UseTask<()>,
}

impl Component for BrowsePanel {
    type State = BrowsePanelState;

    fn init(&self) -> BrowsePanelState {
        let access = RwSignal::new(UNREAD_ACCESS.to_string());
        let limited = RwSignal::new(false);
        let albums = RwSignal::new(Vec::new());
        let album = RwSignal::new(None);
        let listing = RwSignal::new(Listing::default());
        let message = RwSignal::new(String::new());
        let signals = BrowseSignals {
            access,
            limited,
            albums,
            album,
            listing,
            message,
        };
        let action = Arc::new(Mutex::new(None));
        let cell = action.clone();
        let work = use_task(move || {
            let requested = cell.lock().expect("action cell poisoned").take();
            async move {
                let step = spawn_blocking(move || run_browse(requested))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))?;
                if let Some(step) = step {
                    signals.apply(step);
                }
                Ok::<(), JoinFailure>(())
            }
        });
        BrowsePanelState {
            selected: self.selected,
            access,
            limited,
            albums,
            album,
            listing,
            message,
            action,
            work,
        }
    }

    fn build(&self, state: &mut BrowsePanelState) -> impl View<BrowsePanelState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let busy = matches!(state.work.signal().get(), AsyncValue::Loading(_));
        let access = state.access.get();
        let limited = state.limited.get();
        let albums = state.albums.get();
        let selected = state.album.get();
        let listing = state.listing.get();
        let message = state.message.get();

        let mut selector = column();
        selector = selector.child(album_button(state, "All", None, selected.is_none(), busy));
        for album in albums.iter().take(MAX_ALBUM_BUTTONS) {
            let is_selected = selected.as_ref() == Some(&album.id);
            selector = selector.child(album_button(
                state,
                &album_label(album),
                Some(album.id.clone()),
                is_selected,
                busy,
            ));
        }
        if albums.len() > MAX_ALBUM_BUTTONS {
            selector = selector.child(
                text(format!("+{} more albums", albums.len() - MAX_ALBUM_BUTTONS))
                    .size(11.0)
                    .color(muted),
            );
        }

        let ids = listing.ids.clone();
        let key_ids = ids.clone();
        let tapped = state.selected;
        let list = ListView::builder_keyed(
            row_count(ids.len()),
            ROW_EXTENT,
            move |index| row_key(&key_ids, index),
            move |index| thumb_row(row_ids(&ids, index).to_vec(), tapped),
        )
        .scrollbar(ScrollbarStyle::default());

        let mut panel = column()
            .child(text("Browse").size(13.0).color(accent))
            .child(
                text(
                    "Lists the photo library through frust-photo-library: Refresh reads \
                     the access status and the albums, then the first page of 60 assets \
                     as thumbnails. Load more fetches the next page. With limited access \
                     Manage selection reopens the system selection. Tap a thumbnail to \
                     select it for the Originals panel.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(text(access).size(12.0))
            .child(SizedBox(None, Some(4.0)));
        if limited {
            panel = panel
                .child(
                    text("Limited access: only the photos you selected are visible.")
                        .size(11.0)
                        .color(muted),
                )
                .child(manage_button(state, busy))
                .child(SizedBox(None, Some(4.0)));
        }
        panel
            .child(refresh_browse_button(state, busy))
            .child(SizedBox(None, Some(8.0)))
            .child(selector)
            .child(SizedBox(None, Some(8.0)))
            .child(SizedBox(None, Some(LIST_VIEWPORT_H)).child(list))
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(
                        text(if busy {
                            "Loading\u{2026}".to_string()
                        } else {
                            count_line(listing.ids.len(), listing.total)
                        })
                        .size(12.0)
                        .color(muted),
                    )
                    .child(SizedBox(Some(12.0), None))
                    .child(more_button(
                        state,
                        busy || !has_more(listing.ids.len(), listing.total),
                        listing.ids.len() as u64,
                    )),
            )
            .child(text(message).size(11.0).color(muted))
    }
}

/// Shown for the access line before the first Refresh.
const UNREAD_ACCESS: &str = "Photo access: not read yet";

/// One list row: up to [`COLUMNS`] thumbnail cells with a gap between them.
fn thumb_row(
    ids: Vec<AssetId>,
    selected: RwSignal<Option<AssetId>>,
) -> impl View<BrowsePanelState> {
    let mut cells = row();
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            cells = cells.child(SizedBox(Some(GAP), None));
        }
        cells = cells.child(component(ThumbCell {
            id: id.clone(),
            selected,
        }));
    }
    SizedBox(None, Some(ROW_EXTENT)).child(cells)
}

/// Queues `action` and restarts the task.
fn queue(action: &Arc<Mutex<Option<BrowseAction>>>, work: &UseTask<()>, next: BrowseAction) {
    *action.lock().expect("action cell poisoned") = Some(next);
    work.restart();
}

fn refresh_browse_button(state: &BrowsePanelState, busy: bool) -> impl View<BrowsePanelState> {
    let (action, work) = (state.action.clone(), state.work.clone());
    let (access, limited, album) = (state.access, state.limited, state.album);
    button("Refresh", move |_state: &mut BrowsePanelState| {
        let (line, is_limited) = read_access();
        access.set(line);
        limited.set(is_limited);
        queue(&action, &work, BrowseAction::Refresh(album.get_untracked()));
    })
    .style(ButtonStyle::Secondary)
    .disabled(busy)
    .small()
}

fn manage_button(state: &BrowsePanelState, busy: bool) -> impl View<BrowsePanelState> {
    let (action, work, album) = (state.action.clone(), state.work.clone(), state.album);
    button("Manage selection", move |_state: &mut BrowsePanelState| {
        queue(&action, &work, BrowseAction::Manage(album.get_untracked()));
    })
    .disabled(busy)
    .small()
}

fn album_button(
    state: &BrowsePanelState,
    label: &str,
    target: Option<AlbumId>,
    selected: bool,
    busy: bool,
) -> impl View<BrowsePanelState> {
    let (action, work) = (state.action.clone(), state.work.clone());
    button(label.to_string(), move |_state: &mut BrowsePanelState| {
        queue(&action, &work, BrowseAction::Select(target.clone()));
    })
    .style(selector_style(selected))
    .disabled(busy)
    .small()
}

fn more_button(
    state: &BrowsePanelState,
    disabled: bool,
    loaded: u64,
) -> impl View<BrowsePanelState> {
    let (action, work, album) = (state.action.clone(), state.work.clone(), state.album);
    button("Load more", move |_state: &mut BrowsePanelState| {
        queue(
            &action,
            &work,
            BrowseAction::More(album.get_untracked(), loaded),
        );
    })
    .style(ButtonStyle::Secondary)
    .disabled(disabled)
    .small()
}

/// The album **Save copy back** files the exported copy into.
const SAVE_ALBUM: &str = "Frust Playground";

/// The version selector's choices.
const VERSIONS: [(&str, AssetVersion); 2] = [
    ("Original", AssetVersion::Original),
    ("Current", AssetVersion::Current),
];

/// How many leading bytes of the exported file the result line shows.
const HEADER_LEN: usize = 12;

/// The progress bar's full width in logical points.
const BAR_WIDTH: f64 = 240.0;

/// The progress bar's height in logical points.
const BAR_HEIGHT: f64 = 6.0;

/// The asset an export acts on and where it came from: the Browse selection first, then the
/// Picker's most recent asset id.
fn export_target(
    selected: Option<AssetId>,
    picked: Option<AssetId>,
) -> Option<(AssetId, &'static str)> {
    selected
        .map(|id| (id, "Browse selection"))
        .or_else(|| picked.map(|id| (id, "Picker pick")))
}

/// Space-separated lowercase hex of `bytes`.
fn header_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `bytes` as text with every non-printable byte shown as `.`, so a `ftypheic` or `II*` marker
/// reads at a glance.
fn header_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

/// An exported file as the result line reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportReport {
    file_name: String,
    mime: String,
    byte_len: u64,
    header: Vec<u8>,
}

impl ExportReport {
    fn new(exported: &ExportedOriginal, header: Vec<u8>) -> Self {
        Self {
            file_name: exported
                .file_name
                .clone()
                .or_else(|| {
                    exported
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "(unnamed)".to_string()),
            mime: exported
                .mime
                .clone()
                .unwrap_or_else(|| "unknown mime".into()),
            byte_len: exported.byte_len,
            header,
        }
    }

    fn line(&self) -> String {
        format!(
            "{} | {} | {} bytes | first {} bytes: {} ({})",
            self.file_name,
            self.mime,
            self.byte_len,
            self.header.len(),
            header_hex(&self.header),
            header_ascii(&self.header),
        )
    }
}

/// The first [`HEADER_LEN`] bytes of `path` (call inside `spawn_blocking`).
fn read_header(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut header = Vec::with_capacity(HEADER_LEN);
    std::fs::File::open(path)?
        .take(HEADER_LEN as u64)
        .read_to_end(&mut header)?;
    Ok(header)
}

/// The status line while an export runs.
fn export_progress_text(fraction: Option<f64>) -> String {
    match fraction {
        None => "Waiting for the platform\u{2026}".to_string(),
        Some(fraction) => {
            let pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
            format!("Exporting\u{2026} {pct}%")
        }
    }
}

/// The filled and empty widths of the progress bar for `fraction`.
fn bar_split(fraction: f64) -> (f64, f64) {
    let filled = BAR_WIDTH * fraction.clamp(0.0, 1.0);
    (filled, BAR_WIDTH - filled)
}

/// What an Originals button asks the background step to do.
enum OriginalsAction {
    Export(AssetId, ExportOptions),
    /// Save the exported file, into [`SAVE_ALBUM`] or (`false`) no album.
    Save(PathBuf, bool),
}

/// What one background step learned.
enum OriginalsStep {
    Exported(Result<(PathBuf, ExportReport), String>),
    Saved(String),
}

/// The save outcome as one line.
fn save_text(result: Result<(), PhotoError>, album: bool) -> String {
    let target = if album { SAVE_ALBUM } else { "no album" };
    match result {
        Ok(_) => format!("Saved a copy to the library ({target})."),
        Err(err) => pick_error_text(&err),
    }
}

/// Runs one action (call inside `spawn_blocking`). `None` is the mount-time fetch.
fn run_originals_action(action: Option<OriginalsAction>) -> Option<OriginalsStep> {
    Some(match action? {
        OriginalsAction::Export(id, options) => {
            OriginalsStep::Exported(match PhotoLibrary::export_original(&id, &options) {
                Ok(exported) => match read_header(&exported.path) {
                    Ok(header) => {
                        let report = ExportReport::new(&exported, header);
                        Ok((exported.path, report))
                    }
                    Err(err) => Err(format!("error: reading the exported file: {err}")),
                },
                Err(err) => Err(pick_error_text(&err)),
            })
        }
        OriginalsAction::Save(path, album) => {
            let mut request = SaveRequest::new(path);
            if album {
                request = request.album(SAVE_ALBUM);
            }
            OriginalsStep::Saved(save_text(PhotoLibrary::save(&request).map(|_| ()), album))
        }
    })
}

/// The Originals panel's signals, copied into the task future.
/// The newest file an earlier run exported, so a save still has a source after the app restarts.
/// iOS ends the app when Photos access changes, and add-only access cannot export.
fn previous_export() -> Option<PathBuf> {
    let dir = frust::paths::cache_dir()?
        .join("frust-photo-library")
        .join("originals");
    newest_file_in(&dir)
}

/// The newest visible file in `dir`; staging files start with a dot.
fn newest_file_in(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then_some(())?;
            Some((meta.modified().ok()?, entry.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// The result line for a file left by an earlier run; empty when there is none.
fn previous_export_text(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|name| format!("Previous export: {}", name.to_string_lossy()))
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
struct OriginalsSignals {
    progress: RwSignal<Option<f64>>,
    result: RwSignal<String>,
    exported: RwSignal<Option<PathBuf>>,
    saved: RwSignal<String>,
}

impl OriginalsSignals {
    /// Copies a finished step into the signals. Called from the task future, which runs on the
    /// tokio runtime rather than the UI thread. Skipped once the panel is gone.
    fn apply(&self, step: OriginalsStep) {
        if self.result.try_get_untracked().is_none() {
            return;
        }
        match step {
            OriginalsStep::Exported(Ok((path, report))) => {
                self.result.set(report.line());
                self.exported.set(Some(path));
            }
            OriginalsStep::Exported(Err(text)) => {
                self.result.set(text);
                self.exported.set(None);
            }
            OriginalsStep::Saved(line) => self.saved.set(line),
        }
        self.progress.set(None);
    }
}

struct OriginalsPanel {
    /// The Browse grid's selection.
    selected: RwSignal<Option<AssetId>>,
    /// The Picker's most recent asset id.
    picked: RwSignal<Option<AssetId>>,
}

struct OriginalsPanelState {
    selected: RwSignal<Option<AssetId>>,
    picked: RwSignal<Option<AssetId>>,
    /// Index into [`VERSIONS`], written by the version buttons only.
    version: RwSignal<usize>,
    /// Whether a cloud-only original may be downloaded.
    network: RwSignal<bool>,
    /// The pending action; `None` resolves to nothing.
    action: Arc<Mutex<Option<OriginalsAction>>>,
    /// The running step; a press restarts it.
    work: UseTask<()>,
    /// Written by the progress callback on a background thread; read only by the task future.
    progress_cell: Arc<Mutex<Option<f64>>>,
    /// The latest progress fraction, copied from the cell by the task future.
    progress: RwSignal<Option<f64>>,
    /// The running export's token; Cancel takes it, each Export press replaces it.
    token: Arc<Mutex<Option<CancelToken>>>,
    /// The export result line, empty before the first export.
    result: RwSignal<String>,
    /// The last exported file, which **Save copy back** saves. Starts as an earlier run's newest.
    exported: RwSignal<Option<PathBuf>>,
    /// The last save result line.
    saved: RwSignal<String>,
}

impl Component for OriginalsPanel {
    type State = OriginalsPanelState;

    fn init(&self) -> OriginalsPanelState {
        let action: Arc<Mutex<Option<OriginalsAction>>> = Arc::new(Mutex::new(None));
        let cell = action.clone();
        let progress_cell: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
        let poll_cell = progress_cell.clone();
        let previous = previous_export();
        let signals = OriginalsSignals {
            progress: RwSignal::new(None),
            result: RwSignal::new(previous_export_text(previous.as_deref())),
            exported: RwSignal::new(previous),
            saved: RwSignal::new(String::new()),
        };
        let work = use_task(move || {
            let requested = cell.lock().expect("action cell poisoned").take();
            let poll_cell = poll_cell.clone();
            async move {
                let mut handle = spawn_blocking(move || run_originals_action(requested));
                // The progress callback only writes the cell; this loop, in the task future on
                // the tokio runtime, copies it into the signal. Stop copying once the panel is
                // gone.
                while !handle.is_finished() {
                    let latest = *poll_cell.lock().expect("progress cell poisoned");
                    let Some(current) = signals.progress.try_get_untracked() else {
                        break;
                    };
                    if latest != current {
                        signals.progress.set(latest);
                    }
                    let _ = spawn_blocking(move || std::thread::sleep(PROGRESS_POLL)).await;
                }
                let step = (&mut handle)
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))?;
                if let Some(step) = step {
                    signals.apply(step);
                }
                Ok::<(), JoinFailure>(())
            }
        });
        OriginalsPanelState {
            selected: self.selected,
            picked: self.picked,
            version: RwSignal::new(0),
            network: RwSignal::new(true),
            action,
            work,
            progress_cell,
            progress: signals.progress,
            token: Arc::new(Mutex::new(None)),
            result: signals.result,
            exported: signals.exported,
            saved: signals.saved,
        }
    }

    fn build(&self, state: &mut OriginalsPanelState) -> impl View<OriginalsPanelState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let scheme = theme.scheme();
        let (accent, muted) = (scheme.primary, scheme.on_surface_variant);
        let track = scheme.surface_container_highest;

        let busy = matches!(state.work.signal().get(), AsyncValue::Loading(_));
        let target = export_target(state.selected.get(), state.picked.get());
        let selected_version = state.version.get();
        let network = state.network.get();
        let progress = state.progress.get();
        let result = state.result.get();
        let saved = state.saved.get();
        let can_save = state.exported.get().is_some() && !busy;

        let mut versions = row();
        for (index, (label, _)) in VERSIONS.iter().enumerate() {
            let signal = state.version;
            versions = versions.child(
                button(*label, move |_state: &mut OriginalsPanelState| {
                    signal.set(index);
                })
                .style(selector_style(index == selected_version))
                .disabled(busy)
                .small(),
            );
            versions = versions.child(SizedBox(Some(8.0), None));
        }
        let network_signal = state.network;
        let network_toggle = button(
            if network {
                "Network: allowed"
            } else {
                "Network: off"
            },
            move |_state: &mut OriginalsPanelState| {
                network_signal.set(!network_signal.get_untracked());
            },
        )
        .style(selector_style(network))
        .disabled(busy)
        .small();

        let target_line = match &target {
            Some((_, source)) => format!("Target: {source}"),
            None => "Target: none (tap a Browse thumbnail or pick an item)".to_string(),
        };

        let mut body = column();
        if busy {
            body = body.child(text(export_progress_text(progress)).size(12.0).color(muted));
            let (filled, rest) = bar_split(progress.unwrap_or(0.0));
            let mut bar = row();
            if filled > 0.0 {
                bar = bar.child(
                    SizedBox(Some(filled), Some(BAR_HEIGHT))
                        .child(colored_box().fill(accent).radius(BAR_HEIGHT / 2.0)),
                );
            }
            if rest > 0.0 {
                bar = bar.child(
                    SizedBox(Some(rest), Some(BAR_HEIGHT))
                        .child(colored_box().fill(track).radius(BAR_HEIGHT / 2.0)),
                );
            }
            body = body.child(bar);
        }

        column()
            .child(text("Originals").size(13.0).color(accent))
            .child(
                text(
                    "Export copies the selected asset's original file (or its edited \
                     rendition) into the app cache off the UI thread; an iCloud-only \
                     original downloads with progress when the network is allowed, and \
                     Cancel stops it. The result line shows the file's name, mime, size and \
                     first bytes. Save copy back files the exported copy into the library.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(text(target_line).size(12.0))
            .child(SizedBox(None, Some(4.0)))
            .child(text("Version").size(12.0))
            .child(versions)
            .child(SizedBox(None, Some(4.0)))
            .child(network_toggle)
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(export_button(state, target.map(|(id, _)| id), busy))
                    .child(SizedBox(Some(8.0), None))
                    .child(originals_cancel_button(state, busy)),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(body)
            .child(text(result).size(11.0))
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(save_button(state, "Save copy back", true, can_save))
                    .child(SizedBox(Some(8.0), None))
                    .child(save_button(state, "Save (no album)", false, can_save)),
            )
            .child(text(saved).size(11.0).color(muted))
    }
}

fn export_button(
    state: &OriginalsPanelState,
    target: Option<AssetId>,
    busy: bool,
) -> impl View<OriginalsPanelState> {
    let (action, work) = (state.action.clone(), state.work.clone());
    let (progress_cell, progress, token) = (
        state.progress_cell.clone(),
        state.progress,
        state.token.clone(),
    );
    let (version, network) = (state.version, state.network);
    let (result, saved, exported) = (state.result, state.saved, state.exported);
    let disabled = busy || target.is_none();
    button("Export", move |_state: &mut OriginalsPanelState| {
        let Some(id) = target.clone() else {
            return;
        };
        // A fresh token and a cleared progress cell for every export.
        let cancel = CancelToken::new();
        *token.lock().expect("token cell poisoned") = Some(cancel.clone());
        *progress_cell.lock().expect("progress cell poisoned") = None;
        progress.set(None);
        result.set(String::new());
        saved.set(String::new());
        exported.set(None);
        let sink = progress_cell.clone();
        let options = ExportOptions::new()
            .version(VERSIONS[version.get_untracked()].1)
            .allow_network(network.get_untracked())
            .on_progress(move |fraction| {
                *sink.lock().expect("progress cell poisoned") = Some(fraction);
            })
            .cancel(cancel);
        *action.lock().expect("action cell poisoned") = Some(OriginalsAction::Export(id, options));
        work.restart();
    })
    .disabled(disabled)
    .small()
}

fn originals_cancel_button(
    state: &OriginalsPanelState,
    busy: bool,
) -> impl View<OriginalsPanelState> {
    let token = state.token.clone();
    button("Cancel", move |_state: &mut OriginalsPanelState| {
        let current = token.lock().expect("token cell poisoned").clone();
        if let Some(current) = current {
            current.cancel();
        }
    })
    .style(ButtonStyle::Secondary)
    .disabled(!busy)
    .small()
}

fn save_button(
    state: &OriginalsPanelState,
    label: &'static str,
    album: bool,
    enabled: bool,
) -> impl View<OriginalsPanelState> {
    let (action, work, exported, saved) = (
        state.action.clone(),
        state.work.clone(),
        state.exported,
        state.saved,
    );
    button(label, move |_state: &mut OriginalsPanelState| {
        let Some(path) = exported.get_untracked() else {
            return;
        };
        saved.set(String::new());
        *action.lock().expect("action cell poisoned") = Some(OriginalsAction::Save(path, album));
        work.restart();
    })
    .style(ButtonStyle::Secondary)
    .disabled(!enabled)
    .small()
}

/// The shared-preferences key holding the picked folder's grant token.
const FOLDER_TOKEN_KEY: &str = "playground.folder_token";

/// Files per scan page; **Load more** fetches the next page.
const FOLDER_PAGE: u32 = 50;

/// Images decoded for the preview strip after the first page of a scan.
const FOLDER_PREVIEWS: usize = 8;

/// The largest file read for a preview.
const MAX_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;

/// The preview tile's edge in logical points.
const PREVIEW_TILE: f64 = 64.0;

/// The folder's name for display: its last path component, `folder` where the platform has no path.
fn folder_name(grant: &FolderGrant) -> String {
    grant.local_path().and_then(Path::file_name).map_or_else(
        || "folder".to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The grant the panel currently holds, shared with the background steps.
type GrantCell = Arc<Mutex<Option<Arc<FolderGrant>>>>;

/// What a press asks the background step to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoldersAction {
    /// The mount-time step: bring back the stored folder, if any.
    Restore,
    Pick,
    Scan {
        offset: u64,
        recursive: bool,
    },
    Release,
}

/// What a background step produced, applied to the signals by the task future.
enum FoldersStep {
    /// A status line; `clear` also empties the listing and previews.
    Message { text: String, clear: bool },
    /// A scan page, the offset it started at and whether the scan descended into subfolders.
    Page {
        page: FolderPage,
        offset: u64,
        recursive: bool,
    },
}

impl FoldersStep {
    fn message(text: impl Into<String>) -> Self {
        Self::Message {
            text: text.into(),
            clear: false,
        }
    }

    fn cleared(text: impl Into<String>) -> Self {
        Self::Message {
            text: text.into(),
            clear: true,
        }
    }
}

/// The status line for a folder step that failed.
fn folder_error_text(err: &PhotoError) -> String {
    match err {
        PhotoError::Cancelled => "Cancelled".to_string(),
        PhotoError::PermissionDenied => "Folder access was lost. Pick folder again.".to_string(),
        other => format!("error: {other}"),
    }
}

/// Stores `grant`'s token. A restored grant may carry a new token, so this runs after every
/// pick and every restore.
fn persist_token(prefs: &SharedPreferences, grant: &FolderGrant) -> Result<(), String> {
    prefs
        .set_string(FOLDER_TOKEN_KEY, grant.token())
        .map_err(|err| format!("could not store the folder: {err}"))
}

/// Brings back the stored folder into `grants`: the status line to show.
fn restore_step(prefs: &SharedPreferences, grants: &GrantCell) -> FoldersStep {
    let Some(token) = prefs.get_string(FOLDER_TOKEN_KEY) else {
        return FoldersStep::message("No folder picked yet.");
    };
    match FolderGrant::restore(&token) {
        Ok(grant) => {
            let stored = persist_token(prefs, &grant);
            let name = folder_name(&grant);
            *grants.lock().expect("grant cell poisoned") = Some(Arc::new(grant));
            FoldersStep::message(match stored {
                Ok(()) => format!("Restored folder {name}. Scan to list it."),
                Err(text) => format!("Restored folder {name}, but {text}."),
            })
        }
        Err(FileDialogError::AccessLost) => {
            let _ = prefs.remove(FOLDER_TOKEN_KEY);
            FoldersStep::cleared("Folder access was lost. Pick folder again.")
        }
        Err(err) => FoldersStep::message(format!("error: {err}")),
    }
}

/// The picker, then the store (call inside `spawn_blocking`).
fn pick_step(
    prefs: &SharedPreferences,
    grants: &GrantCell,
    cancel: &FolderCancel,
    sink: &Arc<Mutex<Option<FileProgress>>>,
) -> FoldersStep {
    let sink = sink.clone();
    let picked = FileDialog::new()
        .title("Pick folder")
        .on_progress(move |progress| {
            *sink.lock().expect("progress cell poisoned") = Some(progress);
        })
        .cancel(cancel.clone())
        .pick_folder();
    match picked {
        Ok(Some(grant)) => {
            let stored = persist_token(prefs, &grant);
            let name = folder_name(&grant);
            // The previous grant is dropped, not released: on Android a release would revoke
            // the persisted permission, which is the same one when the same folder is picked.
            *grants.lock().expect("grant cell poisoned") = Some(Arc::new(grant));
            FoldersStep::cleared(match stored {
                Ok(()) => format!("Picked folder {name}. Scan to list it."),
                Err(text) => format!("Picked folder {name}, but {text}."),
            })
        }
        Ok(None) => FoldersStep::message("Nothing picked."),
        Err(FileDialogError::Cancelled) => FoldersStep::message("Cancelled"),
        Err(err) => FoldersStep::message(format!("error: {err}")),
    }
}

/// One scan page (call inside `spawn_blocking`). The scan itself has no cancel hook, so a
/// cancel during it discards the page once it returns.
fn scan_step(
    grants: &GrantCell,
    offset: u64,
    recursive: bool,
    cancel: &FolderCancel,
) -> FoldersStep {
    let grant = grants.lock().expect("grant cell poisoned").clone();
    let Some(grant) = grant else {
        return FoldersStep::message("Pick a folder first.");
    };
    let scan = FolderScan::new()
        .recursive(recursive)
        .offset(offset)
        .limit(FOLDER_PAGE);
    let result = PhotoLibrary::scan_folder(&grant, &scan);
    if cancel.is_cancelled() {
        return FoldersStep::message("Cancelled");
    }
    match result {
        Ok(page) => FoldersStep::Page {
            page,
            offset,
            recursive,
        },
        Err(err) => FoldersStep::message(folder_error_text(&err)),
    }
}

/// Gives the folder up and forgets its token (call inside `spawn_blocking`).
fn release_step(prefs: &SharedPreferences, grants: &GrantCell) -> FoldersStep {
    let grant = grants.lock().expect("grant cell poisoned").take();
    let _ = prefs.remove(FOLDER_TOKEN_KEY);
    match grant.map(Arc::try_unwrap) {
        None => FoldersStep::cleared("No folder to release."),
        Some(Ok(grant)) => {
            grant.release();
            FoldersStep::cleared("Released the folder and forgot its token.")
        }
        Some(Err(_)) => FoldersStep::cleared("Forgot the folder token (a scan still held it)."),
    }
}

/// Runs one action (call inside `spawn_blocking`).
fn run_folders_action(
    action: FoldersAction,
    grants: &GrantCell,
    cancel: &FolderCancel,
    sink: &Arc<Mutex<Option<FileProgress>>>,
) -> FoldersStep {
    let prefs = match SharedPreferences::standard() {
        Ok(prefs) => prefs,
        Err(err) => return FoldersStep::message(format!("preferences unavailable: {err}")),
    };
    match action {
        FoldersAction::Restore => restore_step(&prefs, grants),
        FoldersAction::Pick => pick_step(&prefs, grants, cancel, sink),
        FoldersAction::Scan { offset, recursive } => scan_step(grants, offset, recursive, cancel),
        FoldersAction::Release => release_step(&prefs, grants),
    }
}

/// One listed file as the panel shows it.
fn media_line(media: &FolderMedia) -> String {
    let kind = match media.kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        _ => "other",
    };
    let size = media
        .entry
        .size
        .map_or_else(|| "size unknown".to_string(), |b| format!("{b} bytes"));
    format!("{} | {kind} | {size}", media.entry.relative_path)
}

/// The count line under the listing.
fn folder_count_line(loaded: usize, total: u64) -> String {
    format!("Showing {loaded} of {total} file(s)")
}

/// Load more's step: the next page of the listing on screen, scanned with the listing's own
/// recursive flag so the page continues the same result set.
fn load_more_action(loaded: usize, listing_recursive: bool) -> FoldersAction {
    FoldersAction::Scan {
        offset: loaded as u64,
        recursive: listing_recursive,
    }
}

/// The bytes of a listed file, refused past `max_bytes`. Every location goes through
/// `read_to_vec`, so the cap holds for a content URI as well as a path.
fn read_preview_bytes(entry: &FolderEntry, max_bytes: u64) -> Result<Vec<u8>, String> {
    entry.read_to_vec(max_bytes).map_err(|err| err.to_string())
}

/// One preview tile: the file name and its decode result.
#[derive(Clone)]
struct PreviewTile {
    name: String,
    image: Result<ImageSource, String>,
}

/// The first [`FOLDER_PREVIEWS`] images of `items`, decoded off the UI thread. Stops at a cancel
/// or once `phase` (the status line) is gone with the panel.
async fn decode_previews(
    items: &[FolderMedia],
    cancel: &FolderCancel,
    phase: RwSignal<String>,
) -> Vec<PreviewTile> {
    let images: Vec<&FolderMedia> = items
        .iter()
        .filter(|media| media.kind == MediaKind::Image)
        .take(FOLDER_PREVIEWS)
        .collect();
    let mut tiles = Vec::new();
    for (index, media) in images.iter().enumerate() {
        if cancel.is_cancelled() || phase.try_get_untracked().is_none() {
            break;
        }
        phase.set(format!("Decoding image {} of {}", index + 1, images.len()));
        let entry = media.entry.clone();
        let image =
            match spawn_blocking(move || read_preview_bytes(&entry, MAX_PREVIEW_BYTES)).await {
                Ok(Ok(bytes)) => decode_image_async(bytes)
                    .await
                    .map_err(|err| format!("decode error: {err}")),
                Ok(Err(text)) => Err(format!("read error: {text}")),
                Err(err) => Err(err.to_string()),
            };
        tiles.push(PreviewTile {
            name: media.entry.name.clone(),
            image,
        });
    }
    tiles
}

/// The Folders panel's signals, copied into the task future.
#[derive(Clone, Copy)]
struct FoldersSignals {
    /// The status line shown while a step runs.
    phase: RwSignal<String>,
    /// The last step's result line.
    status: RwSignal<String>,
    lines: RwSignal<Vec<String>>,
    /// Whether the listing on screen came from a recursive scan. Load more continues that
    /// scan, so it must not follow the toggle, which may have changed since.
    listing_recursive: RwSignal<bool>,
    total: RwSignal<u64>,
    previews: RwSignal<Vec<PreviewTile>>,
    holds_grant: RwSignal<bool>,
    progress: RwSignal<Option<FileProgress>>,
}

impl FoldersSignals {
    /// Applies a finished step to the signals. Runs in the task future, on the tokio runtime
    /// rather than the UI thread.
    async fn apply(self, step: FoldersStep, cancel: &FolderCancel) {
        match step {
            FoldersStep::Message { text, clear } => {
                self.status.set(text);
                if clear {
                    self.lines.set(Vec::new());
                    self.total.set(0);
                    self.previews.set(Vec::new());
                }
            }
            FoldersStep::Page {
                page,
                offset,
                recursive,
            } => {
                let mut lines = if offset == 0 {
                    Vec::new()
                } else {
                    self.lines.get_untracked()
                };
                lines.extend(page.items.iter().map(media_line));
                self.status.set(folder_count_line(lines.len(), page.total));
                self.total.set(page.total);
                self.lines.set(lines);
                self.listing_recursive.set(recursive);
                if offset == 0 {
                    self.previews.set(Vec::new());
                    let tiles = decode_previews(&page.items, cancel, self.phase).await;
                    if cancel.is_cancelled() {
                        let text = self.status.get_untracked();
                        self.status.set(format!("{text} (previews cancelled)"));
                    }
                    self.previews.set(tiles);
                }
            }
        }
    }
}

/// The status line while a step runs: the pick's progress once it reports, else the phase.
fn folders_busy_text(phase: &str, progress: Option<FileProgress>) -> String {
    match progress {
        Some(p) => loading_text(Some((p.done, p.items, p.fraction))),
        None => phase.to_string(),
    }
}

/// The phase a press starts in.
fn start_phase(action: FoldersAction) -> &'static str {
    match action {
        FoldersAction::Restore => "Restoring the folder\u{2026}",
        FoldersAction::Pick => "Waiting for the picker\u{2026}",
        FoldersAction::Scan { .. } => "Scanning the folder\u{2026}",
        FoldersAction::Release => "Releasing the folder\u{2026}",
    }
}

struct FoldersPanel;

struct FoldersPanelState {
    /// Whether a scan descends into subfolders; written by its toggle only.
    recursive: RwSignal<bool>,
    /// The pending action; a press replaces it.
    action: Arc<Mutex<FoldersAction>>,
    work: UseTask<()>,
    /// The running step's token; Cancel fires it, each press replaces it.
    token: Arc<Mutex<FolderCancel>>,
    /// Written by the pick's progress callback on a background thread.
    progress_cell: Arc<Mutex<Option<FileProgress>>>,
    signals: FoldersSignals,
}

impl Component for FoldersPanel {
    type State = FoldersPanelState;

    fn init(&self) -> FoldersPanelState {
        let grants: GrantCell = Arc::new(Mutex::new(None));
        let action = Arc::new(Mutex::new(FoldersAction::Restore));
        let token = Arc::new(Mutex::new(FolderCancel::new()));
        let progress_cell: Arc<Mutex<Option<FileProgress>>> = Arc::new(Mutex::new(None));
        let signals = FoldersSignals {
            phase: RwSignal::new(start_phase(FoldersAction::Restore).to_string()),
            status: RwSignal::new(String::new()),
            lines: RwSignal::new(Vec::new()),
            listing_recursive: RwSignal::new(false),
            total: RwSignal::new(0),
            previews: RwSignal::new(Vec::new()),
            holds_grant: RwSignal::new(false),
            progress: RwSignal::new(None),
        };
        let (action_in, token_in, cell_in) = (action.clone(), token.clone(), progress_cell.clone());
        let work = use_task(move || {
            // The mount-time run restores; every later run is a press.
            let requested = std::mem::replace(
                &mut *action_in.lock().expect("action cell poisoned"),
                FoldersAction::Restore,
            );
            let cancel = token_in.lock().expect("token cell poisoned").clone();
            let (grants, cell) = (grants.clone(), cell_in.clone());
            async move {
                let (blocking_grants, blocking_cancel, sink) =
                    (grants.clone(), cancel.clone(), cell.clone());
                let mut handle = spawn_blocking(move || {
                    run_folders_action(requested, &blocking_grants, &blocking_cancel, &sink)
                });
                // The pick's callback only writes the cell; copy it into the signal here and
                // stop once the panel is gone.
                while !handle.is_finished() {
                    let latest = *cell.lock().expect("progress cell poisoned");
                    let Some(current) = signals.progress.try_get_untracked() else {
                        break;
                    };
                    if latest != current {
                        signals.progress.set(latest);
                    }
                    let _ = spawn_blocking(move || std::thread::sleep(PROGRESS_POLL)).await;
                }
                let step = (&mut handle)
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))?;
                if signals.status.try_get_untracked().is_none() {
                    return Ok::<(), JoinFailure>(());
                }
                signals.apply(step, &cancel).await;
                signals
                    .holds_grant
                    .set(grants.lock().expect("grant cell poisoned").is_some());
                signals.progress.set(None);
                Ok(())
            }
        });
        FoldersPanelState {
            recursive: RwSignal::new(false),
            action,
            work,
            token,
            progress_cell,
            signals,
        }
    }

    fn build(&self, state: &mut FoldersPanelState) -> impl View<FoldersPanelState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let scheme = theme.scheme();
        let (accent, muted) = (scheme.primary, scheme.on_surface_variant);

        let signals = state.signals;
        let busy = matches!(state.work.signal().get(), AsyncValue::Loading(_));
        let holds = signals.holds_grant.get();
        let recursive = state.recursive.get();
        let listing_recursive = signals.listing_recursive.get();
        let loaded = signals.lines.get();
        let more = has_more(loaded.len(), signals.total.get());
        let status = if busy {
            folders_busy_text(&signals.phase.get(), signals.progress.get())
        } else {
            signals.status.get()
        };

        let mut listing = column();
        for line in &loaded {
            listing = listing.child(text(line.clone()).size(11.0));
        }
        let mut tiles = row();
        for tile in signals.previews.get() {
            tiles = tiles.child(match tile.image {
                Ok(source) => column()
                    .child(
                        SizedBox(Some(PREVIEW_TILE), Some(PREVIEW_TILE))
                            .child(Image(source).fit(ImageFit::Cover)),
                    )
                    .child(text(tile.name).size(9.0).color(muted)),
                Err(message) => column().child(text(message).size(9.0).color(muted)),
            });
            tiles = tiles.child(SizedBox(Some(GAP), None));
        }

        column()
            .child(text("Folders").size(13.0).color(accent))
            .child(
                text(
                    "Pick folder keeps a grant on a folder and stores its token in shared \
                     preferences; the next launch restores it (or asks to pick again when \
                     access was lost). Scan lists its images and videos newest first and \
                     decodes the first eight images. Release gives the folder up.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(press_button(
                        "Pick folder",
                        FoldersAction::Pick,
                        state,
                        busy,
                        false,
                    ))
                    .child(SizedBox(Some(8.0), None))
                    .child(press_button(
                        "Scan",
                        FoldersAction::Scan {
                            offset: 0,
                            recursive,
                        },
                        state,
                        busy,
                        !holds,
                    ))
                    .child(SizedBox(Some(8.0), None))
                    .child(recursive_button(state, recursive))
                    .child(SizedBox(Some(8.0), None))
                    .child(press_button(
                        "Release",
                        FoldersAction::Release,
                        state,
                        busy,
                        !holds,
                    ))
                    .child(SizedBox(Some(8.0), None))
                    .child(folders_cancel_button(state, busy)),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(text(status).size(12.0).color(muted))
            .child(tiles)
            .child(listing)
            .child(row().child(press_button(
                "Load more",
                load_more_action(loaded.len(), listing_recursive),
                state,
                busy,
                !more,
            )))
    }
}

/// A button that starts `action` with a fresh token and cleared progress.
fn press_button(
    label: &'static str,
    action: FoldersAction,
    state: &FoldersPanelState,
    busy: bool,
    unavailable: bool,
) -> impl View<FoldersPanelState> {
    let (cell, token, progress_cell, work, signals) = (
        state.action.clone(),
        state.token.clone(),
        state.progress_cell.clone(),
        state.work.clone(),
        state.signals,
    );
    button(label, move |_state: &mut FoldersPanelState| {
        *token.lock().expect("token cell poisoned") = FolderCancel::new();
        *progress_cell.lock().expect("progress cell poisoned") = None;
        signals.progress.set(None);
        signals.phase.set(start_phase(action).to_string());
        *cell.lock().expect("action cell poisoned") = action;
        work.restart();
    })
    .style(ButtonStyle::Secondary)
    .disabled(busy || unavailable)
    .small()
}

/// The recursive toggle for the next scan.
fn recursive_button(state: &FoldersPanelState, recursive: bool) -> impl View<FoldersPanelState> {
    let signal = state.recursive;
    button(
        if recursive {
            "Recursive: on"
        } else {
            "Recursive: off"
        },
        move |_state: &mut FoldersPanelState| signal.set(!signal.get_untracked()),
    )
    .style(selector_style(recursive))
    .small()
}

/// Fires the running step's token; enabled only while a step runs.
fn folders_cancel_button(state: &FoldersPanelState, busy: bool) -> impl View<FoldersPanelState> {
    let token = state.token.clone();
    button("Cancel", move |_state: &mut FoldersPanelState| {
        let current = token.lock().expect("token cell poisoned").clone();
        current.cancel();
    })
    .style(ButtonStyle::Secondary)
    .disabled(!busy)
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
    fn status_waits_until_the_first_report() {
        assert_eq!(loading_text(None), "Waiting for the picker\u{2026}");
    }

    #[test]
    fn status_shows_count_and_percent() {
        assert_eq!(loading_text(Some((2, 8, 0.25))), "Loading 2 of 8 (25%)");
        assert_eq!(loading_text(Some((0, 3, 0.0))), "Loading 0 of 3 (0%)");
        assert_eq!(loading_text(Some((3, 3, 1.0))), "Loading 3 of 3 (100%)");
    }

    #[test]
    fn cancelled_pick_reads_cancelled() {
        assert_eq!(pick_error_text(&PhotoError::Cancelled), "Cancelled");
        assert!(pick_error_text(&PhotoError::Busy).starts_with("error: "));
    }

    fn ids(names: &[&str]) -> Vec<AssetId> {
        names.iter().map(|n| AssetId::from_raw(*n)).collect()
    }

    #[test]
    fn rows_chunk_four_cells() {
        let all = ids(&["a", "b", "c", "d", "e", "f"]);
        assert_eq!(row_count(0), 0);
        assert_eq!(row_count(4), 1);
        assert_eq!(row_count(6), 2);
        assert_eq!(row_ids(&all, 0), ids(&["a", "b", "c", "d"]).as_slice());
        assert_eq!(row_ids(&all, 1), ids(&["e", "f"]).as_slice());
        assert!(row_ids(&all, 2).is_empty());
    }

    #[test]
    fn row_keys_follow_row_content() {
        let all = ids(&["a", "b", "c", "d", "e"]);
        let grown = ids(&["a", "b", "c", "d", "e", "f"]);
        assert!(row_key(&all, 0) == row_key(&grown, 0));
        assert!(row_key(&all, 1) != row_key(&grown, 1));
        let other = ids(&["x", "b", "c", "d"]);
        assert!(row_key(&all, 0) != row_key(&other, 0));
    }

    #[test]
    fn merge_appends_without_duplicates() {
        let merged = merge_page(&ids(&["a", "b"]), ids(&["b", "c", "c", "d"]));
        assert_eq!(merged, ids(&["a", "b", "c", "d"]));
    }

    #[test]
    fn more_pages_while_fewer_than_total() {
        assert!(has_more(60, 130));
        assert!(!has_more(130, 130));
        assert!(!has_more(0, 0));
        assert_eq!(count_line(60, 130), "Showing 60 of 130 assets");
    }

    #[test]
    fn query_pages_by_page_size_and_album() {
        let q = browse_query(&None, 60);
        assert_eq!(
            (q.offset, q.limit, q.album.is_none()),
            (60, PAGE_SIZE, true)
        );
        let q = browse_query(&Some(AlbumId::from_raw("x")), 0);
        assert_eq!(q.album, Some(AlbumId::from_raw("x")));
    }

    #[test]
    fn access_line_reads_the_status() {
        assert_eq!(
            access_line(Ok(PermissionStatus::Limited)),
            "Photo access: Limited"
        );
        assert!(
            access_line(Err(PermissionError::Unsupported)).starts_with("Photo access: error: ")
        );
    }

    #[test]
    fn thumbnails_are_requested_at_three_times_the_cell() {
        assert_eq!(CELL_EDGE_PX, 228);
        assert_eq!(PREVIEW_EDGE_PX, 480);
    }

    #[test]
    fn no_browse_action_resolves_to_nothing() {
        assert!(run_browse(None).is_none());
    }

    #[test]
    fn any_thumbnail_failure_falls_back_to_the_file() {
        let denied: Result<ImageSource, String> = Err("photo access denied".into());
        let missing: Result<ImageSource, String> = Err("asset not found".into());
        let thumb = Ok(ImageSource::from_rgba8(vec![0; 4], 1, 1));
        assert!(falls_back_to_file(&denied));
        assert!(falls_back_to_file(&missing));
        assert!(!falls_back_to_file(&thumb));
    }

    #[test]
    fn failed_album_switch_commits_no_album() {
        let target = Some(AlbumId::from_raw("x"));
        let mut failed = BrowseStep::default();
        record_page(&target, false, Err(PhotoError::Busy), &mut failed);
        assert!(failed.album.is_none());
        assert!(failed.page.is_none());
        assert!(failed.message.starts_with("error: "));
    }

    #[test]
    fn loaded_album_switch_commits_its_album_with_the_listing() {
        let target = Some(AlbumId::from_raw("x"));
        let page = AssetPage {
            items: Vec::new(),
            total: 0,
        };
        let mut step = BrowseStep::default();
        record_page(&target, false, Ok(page), &mut step);
        assert_eq!(step.album, Some(target));
        assert!(matches!(step.page, Some(PageUpdate::Replace(_))));
    }

    #[test]
    fn appended_page_keeps_the_listing_album() {
        let page = AssetPage {
            items: Vec::new(),
            total: 0,
        };
        let mut step = BrowseStep::default();
        record_page(&None, true, Ok(page), &mut step);
        assert!(step.album.is_none());
        assert!(matches!(step.page, Some(PageUpdate::Append(_))));
    }

    #[test]
    fn target_prefers_the_browse_selection() {
        let a = Some(AssetId::from_raw("a"));
        let b = Some(AssetId::from_raw("b"));
        assert_eq!(
            export_target(a.clone(), b.clone()),
            Some((AssetId::from_raw("a"), "Browse selection"))
        );
        assert_eq!(
            export_target(None, b),
            Some((AssetId::from_raw("b"), "Picker pick"))
        );
        assert_eq!(export_target(None, None), None);
    }

    #[test]
    fn header_shows_hex_and_printable_text() {
        let heic = [
            0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c',
        ];
        assert_eq!(header_hex(&heic), "00 00 00 18 66 74 79 70 68 65 69 63");
        assert_eq!(header_ascii(&heic), "....ftypheic");
        assert_eq!(header_ascii(&[b'I', b'I', b'*', 0]), "II*.");
    }

    #[test]
    fn export_report_line_lists_name_mime_size_and_header() {
        let report = ExportReport {
            file_name: "IMG_1.HEIC".into(),
            mime: "image/heic".into(),
            byte_len: 3,
            header: vec![b'I', b'I', b'*'],
        };
        assert_eq!(
            report.line(),
            "IMG_1.HEIC | image/heic | 3 bytes | first 3 bytes: 49 49 2a (II*)"
        );
    }

    #[test]
    fn header_reads_at_most_twelve_bytes() {
        let dir = std::env::temp_dir().join("frust-playground-originals-header-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("sample.bin");
        std::fs::write(&file, (0u8..40).collect::<Vec<_>>()).expect("write sample");
        assert_eq!(
            read_header(&file).expect("read header"),
            (0u8..12).collect::<Vec<_>>()
        );
        std::fs::remove_file(&file).expect("cleanup");
    }

    #[test]
    fn newest_file_ignores_staging_files_and_empty_dirs() {
        let dir = std::env::temp_dir().join("frust-playground-originals-newest-test");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(newest_file_in(&dir), None);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(".a.jpg.part"), b"staging").expect("write staging");
        assert_eq!(newest_file_in(&dir), None);
        let file = dir.join("a.jpg");
        std::fs::write(&file, b"done").expect("write file");
        assert_eq!(newest_file_in(&dir), Some(file.clone()));
        assert_eq!(previous_export_text(Some(&file)), "Previous export: a.jpg");
        assert_eq!(previous_export_text(None), "");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn export_status_waits_then_shows_percent() {
        assert_eq!(
            export_progress_text(None),
            "Waiting for the platform\u{2026}"
        );
        assert_eq!(export_progress_text(Some(0.426)), "Exporting\u{2026} 43%");
        assert_eq!(export_progress_text(Some(2.0)), "Exporting\u{2026} 100%");
    }

    #[test]
    fn bar_splits_the_track() {
        assert_eq!(bar_split(0.0), (0.0, BAR_WIDTH));
        assert_eq!(bar_split(0.5), (BAR_WIDTH / 2.0, BAR_WIDTH / 2.0));
        assert_eq!(bar_split(3.0), (BAR_WIDTH, 0.0));
    }

    #[test]
    fn no_originals_action_resolves_to_nothing() {
        assert!(run_originals_action(None).is_none());
    }

    #[test]
    fn save_text_names_the_album_and_failures() {
        assert!(save_text(Ok(()), true).contains(SAVE_ALBUM));
        assert!(save_text(Ok(()), false).contains("no album"));
        assert_eq!(save_text(Err(PhotoError::Cancelled), true), "Cancelled");
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

    /// A fresh directory under the temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "frust-playground-folders-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn no_grants() -> GrantCell {
        Arc::new(Mutex::new(None))
    }

    fn message_of(step: &FoldersStep) -> &str {
        match step {
            FoldersStep::Message { text, .. } => text,
            FoldersStep::Page { .. } => "(page)",
        }
    }

    #[test]
    fn restore_without_a_token_asks_for_a_pick() {
        let prefs = SharedPreferences::in_memory();
        let grants = no_grants();
        let step = restore_step(&prefs, &grants);
        assert_eq!(message_of(&step), "No folder picked yet.");
        assert!(grants.lock().unwrap().is_none());
    }

    #[test]
    fn restore_brings_back_the_stored_folder_and_stores_its_token_again() {
        let dir = Scratch::new("restore");
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        let step = restore_step(&prefs, &grants);
        assert!(
            message_of(&step).starts_with("Restored folder"),
            "{}",
            message_of(&step)
        );
        assert!(grants.lock().unwrap().is_some());
        assert!(prefs.get_string(FOLDER_TOKEN_KEY).is_some());
    }

    #[test]
    fn restore_of_lost_access_forgets_the_token_and_asks_again() {
        let prefs = SharedPreferences::in_memory();
        let gone = std::env::temp_dir().join("frust-playground-folders-gone-never-created");
        prefs
            .set_string(FOLDER_TOKEN_KEY, gone.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        let step = restore_step(&prefs, &grants);
        assert!(matches!(step, FoldersStep::Message { clear: true, .. }));
        assert!(message_of(&step).contains("Pick folder again"));
        assert_eq!(prefs.get_string(FOLDER_TOKEN_KEY), None);
        assert!(grants.lock().unwrap().is_none());
    }

    #[test]
    fn release_drops_the_grant_and_the_stored_token() {
        let dir = Scratch::new("release");
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        restore_step(&prefs, &grants);
        let step = release_step(&prefs, &grants);
        assert!(matches!(step, FoldersStep::Message { clear: true, .. }));
        assert!(grants.lock().unwrap().is_none());
        assert_eq!(prefs.get_string(FOLDER_TOKEN_KEY), None);
    }

    #[test]
    fn scan_without_a_folder_says_so() {
        let step = scan_step(&no_grants(), 0, false, &FolderCancel::new());
        assert_eq!(message_of(&step), "Pick a folder first.");
    }

    #[test]
    fn scan_lists_the_media_of_the_folder() {
        let dir = Scratch::new("scan");
        std::fs::write(dir.0.join("a.png"), b"x").unwrap();
        std::fs::write(dir.0.join("notes.txt"), b"x").unwrap();
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        restore_step(&prefs, &grants);
        match scan_step(&grants, 0, false, &FolderCancel::new()) {
            FoldersStep::Page {
                page,
                offset,
                recursive,
            } => {
                assert_eq!((offset, page.total, page.items.len()), (0, 1, 1));
                assert!(!recursive);
                assert_eq!(media_line(&page.items[0]), "a.png | image | 1 bytes");
            }
            other => panic!("expected a page, got {}", message_of(&other)),
        }
    }

    #[test]
    fn a_page_records_the_recursive_flag_it_was_scanned_with() {
        let dir = Scratch::new("scan-recursive");
        std::fs::write(dir.0.join("a.png"), b"x").unwrap();
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        restore_step(&prefs, &grants);
        match scan_step(&grants, 0, true, &FolderCancel::new()) {
            FoldersStep::Page { recursive, .. } => assert!(recursive),
            other => panic!("expected a page, got {}", message_of(&other)),
        }
    }

    #[test]
    fn load_more_continues_the_listing_with_its_own_flag() {
        assert_eq!(
            load_more_action(50, true),
            FoldersAction::Scan {
                offset: 50,
                recursive: true,
            }
        );
        assert_eq!(
            load_more_action(50, false),
            FoldersAction::Scan {
                offset: 50,
                recursive: false,
            }
        );
    }

    #[test]
    fn preview_read_refuses_a_file_past_the_cap() {
        let dir = Scratch::new("preview-cap");
        std::fs::write(dir.0.join("big.png"), [7u8; 16]).unwrap();
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        restore_step(&prefs, &grants);
        let FoldersStep::Page { page, .. } = scan_step(&grants, 0, false, &FolderCancel::new())
        else {
            panic!("expected a page");
        };
        let entry = &page.items[0].entry;
        assert!(read_preview_bytes(entry, 15).is_err());
        assert_eq!(read_preview_bytes(entry, 16).unwrap(), vec![7u8; 16]);
    }

    #[test]
    fn a_cancel_during_the_scan_discards_the_page() {
        let dir = Scratch::new("scan-cancel");
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        restore_step(&prefs, &grants);
        let cancel = FolderCancel::new();
        cancel.cancel();
        let step = scan_step(&grants, 0, false, &cancel);
        assert_eq!(message_of(&step), "Cancelled");
    }

    #[test]
    fn folder_errors_read_plainly() {
        assert_eq!(folder_error_text(&PhotoError::Cancelled), "Cancelled");
        assert!(folder_error_text(&PhotoError::PermissionDenied).contains("Pick folder again"));
        assert!(folder_error_text(&PhotoError::Busy).starts_with("error: "));
    }

    #[test]
    fn busy_text_prefers_the_pick_progress_over_the_phase() {
        assert_eq!(
            folders_busy_text("Scanning the folder\u{2026}", None),
            "Scanning the folder\u{2026}"
        );
        assert_eq!(
            start_phase(FoldersAction::Pick),
            "Waiting for the picker\u{2026}"
        );
        assert_eq!(folder_count_line(50, 120), "Showing 50 of 120 file(s)");
    }
}
