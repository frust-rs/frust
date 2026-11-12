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
//! decoders are compiled in, so the preview first tries them; when that fails it falls back to
//! `PhotoLibrary::decode_file` (the plugin's `decode` feature: ImageIO on iOS and macOS,
//! `ImageDecoder` on Android), so a picked HEIC previews there. Other desktops report
//! `Unsupported` from the fallback, and the preview then shows the original decode error.
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
//! first and decodes the copied file only when that fails.
//!
//! A **Cursor paging** toggle beside Refresh switches Browse to `PhotoLibrary::assets_after`:
//! each page keeps the returned `next` cursor for **Load more**, the count line drops the total
//! (cursor paging computes none), and Load more is disabled once `next` is `None`. Off, Browse
//! pages by offset exactly as above. Switching the toggle or the album reloads the first page,
//! which drops the listing and its cursor; Load more continues in the mode the listing on screen
//! was loaded with. A failed cursor page (an `InvalidRequest` for an evicted iOS snapshot, say)
//! shows its text, and the next Refresh starts again from the first page.
//!
//! A Changes panel below Browse drives the library change feed. **Save token** reads
//! `PhotoLibrary::current_change_token` and stores its bytes as hex in shared preferences (key
//! `playground.change_token`), so the token survives a relaunch; the mount-time step reads it
//! back, and a line shows whether one is stored and its byte length, never its bytes. **Check**
//! calls `PhotoLibrary::changes_since` with the stored token, shows `Delta: +inserted ~updated
//! -deleted` or `Full rescan` (or the typed error's `Display` text), and stores the `next`
//! token. Each step runs in [`frust::spawn_blocking`] behind a [`use_task`] and its result
//! reaches `build` through the task's value. **Watch** subscribes with
//! `PhotoLibrary::on_library_changed`, off the UI thread; the [`ChangeSubscription`] lives in
//! [`LibraryWatch`], held in the app state so a watch outlives a section switch, and **Stop**
//! drops it. The callback runs on an arbitrary thread and only bumps the watch's atomic count;
//! the watch task polls the count and copies it into a signal while the watch lasts (the shape
//! the Picker uses for progress). A subscribe error shows as text.
//!
//! An Originals panel below the Changes panel acts on one asset: the thumbnail tapped in the Browse grid
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
//!
//! **Decode (2048)** and **Decode (full, cap 16384)** call `PhotoLibrary::decode_asset` on the
//! same target, in [`frust::spawn_blocking`] behind their own [`use_task`] (the call refuses the
//! UI thread on iOS and Android). The decoded pixels become one [`ImageSource`] inside the
//! background step, and the task's value carries it with the source and output sizes, the colour
//! space and an EXIF summary, so `build` never creates an image and no signal is set inside the
//! future. A failed decode shows the typed error. **Cancel decode** cancels the decode's token.
//!
//! Browse queries with `AssetQuery::with_details`, so each cell badges its content type (a MIME
//! type on Android, a uniform type identifier on iOS, both mapped by [`type_badge`]) and marks a
//! favourite. Cells load through `PhotoLibrary::thumbnail_shared` with a [`CancelToken`] their
//! teardown cancels, and keep one [`ImageSource`] in the task value. The counters line shows
//! the thumbnails served from the cache (a lower bound) and the cancelled requests. **Metadata**
//! reads `PhotoLibrary::asset_metadata` for the target in its own task, network off unless
//! toggled; **Cancel metadata** cancels that read's own token.
//! Button rows use `wrap()` so every button stays reachable on a phone width.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use frust::authoring::Rect;
use frust::{
    Align, Alignment, AsyncValue, ButtonStyle, ChildKey, Component, EdgeInsets, GestureDetector,
    Get, GetUntracked, Image, ImageFit, ImageSource, ListView, Padding, RwSignal, ScrollbarStyle,
    Set, SizedBox, Theme, Update, UseTask, View, button, colored_box, column, component,
    decode_image_async, either, on_cleanup, row, spawn_blocking, stack, text, use_context,
    use_task, views, wrap,
};
use frust_file_dialogs::{
    CancelToken as FolderCancel, FileDialog, FileDialogError, FileProgress, FolderGrant,
};
use frust_permissions::{
    Permission, PermissionError, PermissionStatus, PhotoAccess, open_settings, request, status,
};
use frust_photo_library::folder::FolderEntry;
use frust_photo_library::{
    Album, AlbumId, Asset, AssetCursor, AssetCursorPage, AssetId, AssetPage, AssetQuery,
    AssetVersion, CancelToken, ChangeSubscription, ChangeToken, DecodeOptions, ExifData,
    ExportOptions, ExportedOriginal, FolderMedia, FolderPage, FolderScan, ImageColorSpace,
    LibraryChanges, MediaFilter, MediaKind, MetadataOptions, PhotoError, PhotoLibrary, PickOptions,
    PickProgress, PickedItem, SaveRequest, Thumbnail, ThumbnailMode, ThumbnailRequest,
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
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(PhotosPage {
        watch: state.photo_watch.clone(),
    })
}

struct PhotosPage {
    /// The app-held library-change watch, handed to the Changes panel.
    watch: LibraryWatch,
}

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
                .child(component(ChangesPanel {
                    watch: self.watch.clone(),
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
/// When that decode fails, the native `decode_file` gets the file before the error is shown, so a
/// HEIC pick previews where the platform decodes it.
async fn file_preview(path: PathBuf) -> Result<Result<ImageSource, String>, JoinFailure> {
    let read_path = path.clone();
    let bytes = spawn_blocking(move || std::fs::read(read_path))
        .await
        .map_err(|err| JoinFailure(err.to_string()))?;
    Ok(match bytes {
        Ok(bytes) => match decode_image_async(bytes).await {
            Ok(source) => Ok(source),
            Err(err) => {
                let native = spawn_blocking(move || native_preview(&path))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))?;
                native.map_err(|_| format!("preview decode error: {err}"))
            }
        },
        Err(err) => Err(format!("preview read error: {err}")),
    })
}

/// The longest side of a native-decoded preview, in pixels.
const PREVIEW_DECODE_CAP: u32 = 1024;

/// The picked file decoded natively and capped at [`PREVIEW_DECODE_CAP`] (call inside
/// `spawn_blocking`).
fn native_preview(path: &Path) -> Result<ImageSource, PhotoError> {
    PhotoLibrary::decode_file(path, &DecodeOptions::new(PREVIEW_DECODE_CAP))
        .map(|image| ImageSource::from_rgba8(image.rgba, image.width, image.height))
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

        let mut filters = wrap().spacing(8.0).run_spacing(8.0);
        for (index, (label, _)) in FILTERS.iter().enumerate() {
            let signal = state.filter;
            filters = filters.child(
                button(*label, move |_state: &mut PickerPanelState| {
                    signal.set(index);
                })
                .style(selector_style(index == selected_filter))
                .small(),
            );
        }
        let mut limits = wrap().spacing(8.0).run_spacing(8.0);
        for (index, (label, _)) in LIMITS.iter().enumerate() {
            let signal = state.limit;
            limits = limits.child(
                button(*label, move |_state: &mut PickerPanelState| {
                    signal.set(index);
                })
                .style(selector_style(index == selected_limit))
                .small(),
            );
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
                     image is previewed (PNG and JPEG decode; HEIC, WebP and GIF fall back \
                     to the native decode where the platform has one, and an item with a \
                     library asset id previews through the native thumbnail first). Share \
                     picked sends the files; \
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
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(pick_button(state, busy))
                    .child(share_button(state, paths))
                    .child(clear_button(state, busy))
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

/// One grid asset: its id and what its cell shows besides the thumbnail.
#[derive(Clone, Debug, PartialEq)]
struct GridAsset {
    id: AssetId,
    /// The type badge from the asset's content type; `None` when it is unknown or unmapped.
    badge: Option<&'static str>,
    /// Whether the cell shows the favourite mark.
    favorite: bool,
}

impl GridAsset {
    /// The cell data of an asset id and the details of its listing.
    fn new(id: AssetId, content_type: Option<&str>, is_favorite: Option<bool>) -> Self {
        Self {
            badge: type_badge(content_type),
            favorite: is_favorite == Some(true),
            id,
        }
    }

    /// The cell data of a listed asset. The badge needs the query's details.
    fn from_asset(asset: Asset) -> Self {
        Self::new(asset.id, asset.content_type.as_deref(), asset.is_favorite)
    }
}

/// The badge of a content type. Android reports a MIME type (`image/heic`) and iOS a uniform
/// type identifier (`public.heic`); both shapes map to the same badge. `None` for a type the
/// grid has no badge for.
fn type_badge(content_type: Option<&str>) -> Option<&'static str> {
    const RAW_MARKERS: [&str; 10] = [
        "raw",
        "dng",
        "x-sony-arw",
        "x-canon-cr",
        "x-nikon-nef",
        "x-olympus-orf",
        "x-fuji-raf",
        "x-panasonic-rw2",
        "x-pentax-pef",
        "x-samsung-srw",
    ];
    let kind = content_type?.to_ascii_lowercase();
    let has = |needle: &str| kind.contains(needle);
    if has("heic") || has("heif") {
        Some("HEIC")
    } else if RAW_MARKERS.iter().any(|marker| has(marker)) {
        Some("RAW")
    } else if has("jpeg") || has("jpg") {
        Some("JPEG")
    } else if has("png") {
        Some("PNG")
    } else if has("gif") {
        Some("GIF")
    } else if has("webp") {
        Some("WEBP")
    } else if has("quicktime") {
        Some("MOV")
    } else if has("mp4") || has("mpeg-4") {
        Some("MP4")
    } else {
        None
    }
}

/// The assets loaded so far for the selected album, and how the next page continues them.
#[derive(Clone)]
struct Listing {
    ids: Arc<Vec<GridAsset>>,
    /// The library's total for the album under offset paging; `None` for a cursor-paged
    /// listing, which has no total.
    total: Option<u64>,
    /// Where the next cursor page starts; `None` for an offset-paged listing and after the
    /// last cursor page.
    next: Option<AssetCursor>,
}

impl Default for Listing {
    fn default() -> Self {
        Self {
            ids: Arc::new(Vec::new()),
            total: Some(0),
            next: None,
        }
    }
}

/// How Browse pages through the library: by offset with a total (the default), or by
/// `PhotoLibrary::assets_after` cursor with none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paging {
    Offset,
    Cursor,
}

/// List rows needed for `count` assets.
fn row_count(count: usize) -> usize {
    count.div_ceil(COLUMNS)
}

/// The asset ids of list row `row`.
fn row_ids(ids: &[GridAsset], row: usize) -> &[GridAsset] {
    let start = (row * COLUMNS).min(ids.len());
    let end = (start + COLUMNS).min(ids.len());
    &ids[start..end]
}

/// A row's identity: the ids of its assets, so a row keeps its thumbnail tasks across a Load
/// more and gets fresh ones when an album switch puts other assets in the same slot.
fn row_key(ids: &[GridAsset], row: usize) -> ChildKey {
    let names: Vec<&str> = row_ids(ids, row)
        .iter()
        .map(|asset| asset.id.as_str())
        .collect();
    ChildKey::new(names)
}

/// `existing` followed by the `page` ids it does not already hold.
fn merge_page(existing: &[GridAsset], page: Vec<GridAsset>) -> Vec<GridAsset> {
    let mut seen: HashSet<AssetId> = existing.iter().map(|asset| asset.id.clone()).collect();
    let mut merged = existing.to_vec();
    merged.extend(
        page.into_iter()
            .filter(|asset| seen.insert(asset.id.clone())),
    );
    merged
}

/// The grid entries of a page's assets.
fn page_ids(items: Vec<Asset>) -> Vec<GridAsset> {
    items.into_iter().map(GridAsset::from_asset).collect()
}

/// Whether another page can exist: fewer assets loaded than the total.
fn has_more(loaded: usize, total: u64) -> bool {
    (loaded as u64) < total
}

/// The count line under the list.
fn count_line(loaded: usize, total: u64) -> String {
    format!("Showing {loaded} of {total} assets")
}

/// The count line under a cursor-paged list, which has no total.
fn cursor_count_line(loaded: usize) -> String {
    format!("Showing {loaded} assets (cursor paging, no total)")
}

/// The count line for `listing`, in the paging mode it was loaded with.
fn listing_line(listing: &Listing) -> String {
    match listing.total {
        Some(total) => count_line(listing.ids.len(), total),
        None => cursor_count_line(listing.ids.len()),
    }
}

/// Load more's step for `listing` of `album`, in the paging mode the listing was loaded with
/// (not the toggle's, which may have changed since a failed switch); `None` when there is no
/// next page.
fn more_action(album: &Option<AlbumId>, listing: &Listing) -> Option<BrowseAction> {
    let loaded = listing.ids.len();
    match listing.total {
        Some(total) => {
            has_more(loaded, total).then(|| BrowseAction::More(album.clone(), loaded as u64))
        }
        None => listing
            .next
            .clone()
            .map(|cursor| BrowseAction::After(album.clone(), cursor)),
    }
}

/// The paging toggle's label.
fn paging_label(paging: Paging) -> &'static str {
    match paging {
        Paging::Offset => "Cursor paging: off",
        Paging::Cursor => "Cursor paging: on",
    }
}

/// The query for the page starting at `offset`.
fn browse_query(album: &Option<AlbumId>, offset: u64) -> AssetQuery {
    let query = AssetQuery::new()
        .limit(PAGE_SIZE)
        .offset(offset)
        .with_details();
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
    Refresh(Option<AlbumId>, Paging),
    /// The first page of another album, or of the same one in the other paging mode.
    Select(Option<AlbumId>, Paging),
    /// The offset page after the `offset` assets already loaded.
    More(Option<AlbumId>, u64),
    /// The cursor page after `cursor`.
    After(Option<AlbumId>, AssetCursor),
    /// Present the limited-selection UI, then re-read access and re-list.
    Manage(Option<AlbumId>, Paging),
}

/// One loaded page in either paging mode.
struct LoadedPage {
    items: Vec<Asset>,
    /// The album's total under offset paging; `None` for a cursor page.
    total: Option<u64>,
    /// Where the next cursor page starts; always `None` for an offset page.
    next: Option<AssetCursor>,
}

impl From<AssetPage> for LoadedPage {
    fn from(page: AssetPage) -> Self {
        Self {
            items: page.items,
            total: Some(page.total),
            next: None,
        }
    }
}

impl From<AssetCursorPage> for LoadedPage {
    fn from(page: AssetCursorPage) -> Self {
        Self {
            items: page.items,
            total: None,
            next: page.next,
        }
    }
}

/// How a loaded page combines with the current listing.
enum PageUpdate {
    Replace(LoadedPage),
    Append(LoadedPage),
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
        Ok(page) => record_loaded(album, append, page.into(), step),
        Err(err) => step.message = format!("error: {err}"),
    }
}

/// [`record_page`] for a cursor page. A failure keeps the listing and its cursor; an
/// `InvalidRequest` (a cursor whose iOS snapshot was evicted, or one made for another query)
/// says that Refresh starts over from the first page.
fn record_cursor_page(
    album: &Option<AlbumId>,
    append: bool,
    result: Result<AssetCursorPage, PhotoError>,
    step: &mut BrowseStep,
) {
    match result {
        Ok(page) => record_loaded(album, append, page.into(), step),
        Err(err) => step.message = cursor_error_text(&err),
    }
}

/// A failed cursor page as one line: its `Display` text, and for an `InvalidRequest` how to
/// start over.
fn cursor_error_text(err: &PhotoError) -> String {
    match err {
        PhotoError::InvalidRequest { .. } => {
            format!("error: {err}. Refresh starts again from the first page.")
        }
        other => format!("error: {other}"),
    }
}

/// Records a loaded page into `step`; see [`record_page`].
fn record_loaded(album: &Option<AlbumId>, append: bool, page: LoadedPage, step: &mut BrowseStep) {
    if append {
        step.page = Some(PageUpdate::Append(page));
    } else {
        step.page = Some(PageUpdate::Replace(page));
        step.album = Some(album.clone());
    }
}

/// Loads the first page of `album` in `paging` mode into `step`. A cursor first page starts a
/// new cursor session, so it drops whatever cursor the listing held.
fn load_first_page(album: &Option<AlbumId>, paging: Paging, step: &mut BrowseStep) {
    match paging {
        Paging::Offset => load_page(album, 0, false, step),
        Paging::Cursor => record_cursor_page(
            album,
            false,
            PhotoLibrary::assets_after(&browse_query(album, 0), None),
            step,
        ),
    }
}

/// Albums, then the first page, into `step` (stops at the first failure).
fn load_albums_and_first_page(album: &Option<AlbumId>, paging: Paging, step: &mut BrowseStep) {
    match PhotoLibrary::albums() {
        Ok(albums) => {
            step.albums = Some(albums);
            load_first_page(album, paging, step);
        }
        Err(err) => step.message = format!("error: {err}"),
    }
}

/// Runs one action (call inside `spawn_blocking`). `None` is the mount-time fetch.
fn run_browse(action: Option<BrowseAction>) -> Option<BrowseStep> {
    let mut step = BrowseStep::default();
    match action? {
        BrowseAction::Refresh(album, paging) => {
            load_albums_and_first_page(&album, paging, &mut step)
        }
        BrowseAction::Select(album, paging) => load_first_page(&album, paging, &mut step),
        BrowseAction::More(album, offset) => load_page(&album, offset, true, &mut step),
        BrowseAction::After(album, cursor) => record_cursor_page(
            &album,
            true,
            PhotoLibrary::assets_after(&browse_query(&album, 0), Some(&cursor)),
            &mut step,
        ),
        BrowseAction::Manage(album, paging) => match PhotoLibrary::manage_limited_selection() {
            Ok(()) => {
                step.access = Some(read_access());
                load_albums_and_first_page(&album, paging, &mut step);
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
                next: page.next,
            }),
            Some(PageUpdate::Append(page)) => {
                let current = self.listing.get_untracked();
                self.listing.set(Listing {
                    ids: Arc::new(merge_page(&current.ids, page_ids(page.items))),
                    total: page.total,
                    next: page.next,
                });
            }
            None => {}
        }
        self.message.set(step.message);
    }
}

/// Thumbnail counters for the device gate, shared by every cell. Atomics, not signals: the
/// cells count on a background thread, and [`ThumbStatsLine`] reads them through a task.
struct ThumbStats {
    /// Served by the cache (the same shared pixels as an earlier request for the asset).
    cached: AtomicU64,
    /// Loaded from the platform.
    loaded: AtomicU64,
    /// Requests that ended `Cancelled`, because their cell was torn down.
    cancelled: AtomicU64,
    /// Requests that failed for another reason.
    failed: AtomicU64,
}

static THUMB_STATS: ThumbStats = ThumbStats {
    cached: AtomicU64::new(0),
    loaded: AtomicU64::new(0),
    cancelled: AtomicU64::new(0),
    failed: AtomicU64::new(0),
};

/// The pixels last served per asset, weakly. A later serve of the same allocation was a cache
/// hit; the cache is the only owner that outlives a cell.
static SERVED: Mutex<Option<HashMap<AssetId, Weak<Thumbnail>>>> = Mutex::new(None);

/// Locks a cell through a poisoned lock: a panic on one worker must not make every later
/// cell panic during build, and the guarded value is still usable.
fn lock_cell<T>(cell: &Mutex<T>) -> MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Entries after which [`SERVED`] starts over, so it stays small on a long scroll.
const SERVED_LIMIT: usize = 4096;

/// How one thumbnail request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbServe {
    Cached,
    Loaded,
    Cancelled,
    Failed,
}

/// Whether `served` is the allocation `previous` still points at, which is how a cache hit
/// looks from outside: the cache hands out a clone of the `Arc` it holds.
fn is_cache_hit(previous: Option<&Weak<Thumbnail>>, served: &Arc<Thumbnail>) -> bool {
    previous
        .and_then(Weak::upgrade)
        .is_some_and(|earlier| Arc::ptr_eq(&earlier, served))
}

/// Classifies a served thumbnail against the last one served for `id`, and remembers it.
fn note_served(
    seen: &mut HashMap<AssetId, Weak<Thumbnail>>,
    id: &AssetId,
    served: &Arc<Thumbnail>,
) -> ThumbServe {
    if seen.len() >= SERVED_LIMIT {
        seen.clear();
    }
    let hit = is_cache_hit(seen.get(id), served);
    seen.insert(id.clone(), Arc::downgrade(served));
    if hit {
        ThumbServe::Cached
    } else {
        ThumbServe::Loaded
    }
}

/// The serve kind of a failed request.
fn failure_serve(err: &PhotoError) -> ThumbServe {
    match err {
        PhotoError::Cancelled => ThumbServe::Cancelled,
        _ => ThumbServe::Failed,
    }
}

/// A point-in-time copy of the counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ThumbCounts {
    cached: u64,
    loaded: u64,
    cancelled: u64,
    failed: u64,
}

impl ThumbCounts {
    /// The counters as the visible line.
    fn line(&self) -> String {
        format!(
            "Thumbnails: {} from cache (lower bound), {} loaded, {} cancelled, {} failed",
            self.cached, self.loaded, self.cancelled, self.failed
        )
    }
}

impl ThumbStats {
    fn record(&self, serve: ThumbServe) {
        let counter = match serve {
            ThumbServe::Cached => &self.cached,
            ThumbServe::Loaded => &self.loaded,
            ThumbServe::Cancelled => &self.cancelled,
            ThumbServe::Failed => &self.failed,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> ThumbCounts {
        ThumbCounts {
            cached: self.cached.load(Ordering::Relaxed),
            loaded: self.loaded.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

/// One cell's thumbnail as an image source (call inside `spawn_blocking`). The pixels come
/// shared from the cache and `token` abandons the request; the source is made once here, so a
/// build only clones it.
fn load_cell_thumbnail(id: &AssetId, token: &CancelToken) -> Result<ImageSource, String> {
    let request = ThumbnailRequest::new(CELL_EDGE_PX, CELL_EDGE_PX, ThumbnailMode::Fill);
    match PhotoLibrary::thumbnail_shared(id, &request, token) {
        Ok(thumb) => {
            let serve = {
                let mut guard = lock_cell(&SERVED);
                note_served(guard.get_or_insert_with(HashMap::new), id, &thumb)
            };
            THUMB_STATS.record(serve);
            Ok(ImageSource::from_rgba8(
                thumb.rgba.clone(),
                thumb.width,
                thumb.height,
            ))
        }
        Err(err) => {
            THUMB_STATS.record(failure_serve(&err));
            Err(err.to_string())
        }
    }
}

/// One thumbnail cell. Its task is keyed by the asset id: the cell is built for one asset and
/// requests that asset's thumbnail once, off the UI thread, with a token of its own that
/// teardown cancels.
struct ThumbCell {
    asset: GridAsset,
    /// Set to this cell's asset on a tap; the cell insets itself while it is the selection.
    selected: RwSignal<Option<AssetId>>,
}

struct ThumbCellState {
    /// The one image source for this asset, in the task's value.
    thumb: UseTask<Result<ImageSource, String>>,
}

impl Component for ThumbCell {
    type State = ThumbCellState;

    fn init(&self) -> Self::State {
        let token = CancelToken::new();
        let on_teardown = token.clone();
        on_cleanup(move || on_teardown.cancel());
        let id = self.asset.id.clone();
        let thumb = use_task(move || {
            let (id, token) = (id.clone(), token.clone());
            async move {
                spawn_blocking(move || load_cell_thumbnail(&id, &token))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))
            }
        });
        ThumbCellState { thumb }
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let scheme = theme.scheme();
        let value = state.thumb.signal().get();
        let (source, failed) = match value {
            AsyncValue::Ready(Ok(source)) => (Some(source), false),
            AsyncValue::Ready(Err(_)) | AsyncValue::Error(_) => (None, true),
            _ => (None, false),
        };
        let (empty, error_fill) = (scheme.surface_container_highest, scheme.error_container);
        let (pill, pill_text) = (scheme.primary, scheme.on_primary);
        let is_selected = self.selected.get().as_ref() == Some(&self.asset.id);
        let target = self.asset.id.clone();
        let selected = self.selected;
        let content = move || Self::picture(source.clone(), failed, empty, error_fill);
        let inset_content = content.clone();
        let (badge, favorite) = (self.asset.badge, self.asset.favorite);
        GestureDetector(
            SizedBox(Some(CELL), Some(CELL)).child(
                stack()
                    .child(either(
                        is_selected,
                        move || Padding(EdgeInsets::all(SELECTED_INSET), inset_content()),
                        move || Padding(EdgeInsets::all(0.0), content()),
                    ))
                    .when_some(badge, |layers, label| {
                        layers.child(Align(
                            Alignment::BOTTOM_RIGHT,
                            Self::mark(label, BADGE_W, pill, pill_text),
                        ))
                    })
                    .when(favorite, |layers| {
                        layers.child(Align(
                            Alignment::TOP_LEFT,
                            Self::mark("FAV", FAV_W, pill, pill_text),
                        ))
                    }),
            ),
        )
        .on_tap(move |_state: &mut Self::State| selected.set(Some(target.clone())))
    }
}

/// The type badge's width, in logical points.
const BADGE_W: f64 = 34.0;

/// The favourite mark's width, in logical points.
const FAV_W: f64 = 28.0;

/// A mark's height, in logical points.
const MARK_H: f64 = 14.0;

/// The inset around the selected cell's thumbnail, in logical points.
const SELECTED_INSET: f64 = 8.0;

impl ThumbCell {
    /// A cell's picture: the thumbnail, or a flat placeholder (tinted when its load failed).
    fn picture(
        source: Option<ImageSource>,
        failed: bool,
        empty: frust::Color,
        error_fill: frust::Color,
    ) -> impl View<ThumbCellState> {
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

    /// A small pill carrying `label`, for the type badge and the favourite mark.
    fn mark(
        label: &'static str,
        width: f64,
        fill: frust::Color,
        ink: frust::Color,
    ) -> impl View<ThumbCellState> {
        SizedBox(Some(width), Some(MARK_H)).child(
            stack()
                .child(colored_box().fill(fill).radius(3.0))
                .child(Align(Alignment::CENTER, text(label).size(9.0).color(ink))),
        )
    }
}

/// The live thumbnail counters: one task whose value is a snapshot, re-read by **Refresh
/// counters**. The cells count on background threads, so nothing sets a signal for them.
struct ThumbStatsLine;

impl Component for ThumbStatsLine {
    type State = UseTask<ThumbCounts>;

    fn init(&self) -> Self::State {
        use_task(|| async { Ok::<ThumbCounts, JoinFailure>(THUMB_STATS.snapshot()) })
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let muted = theme.scheme().on_surface_variant;
        let counts = state.signal().get().value().copied().unwrap_or_default();
        let task = state.clone();
        wrap()
            .spacing(8.0)
            .run_spacing(4.0)
            .child(text(counts.line()).size(11.0).color(muted))
            .child(
                button("Refresh counters", move |_state: &mut Self::State| {
                    task.restart();
                })
                .style(ButtonStyle::Secondary)
                .small(),
            )
    }
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
    /// The paging mode the next first page loads in; written by the toggle only.
    paging: RwSignal<Paging>,
    /// The loaded assets, the total (offset paging) and the next cursor (cursor paging).
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
            paging: RwSignal::new(Paging::Offset),
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
        let paging = state.paging.get();
        let listing = state.listing.get();
        let message = state.message.get();
        let next_page = more_action(&selected, &listing);

        // Switching the mode reloads the first page of the selected album, which drops the
        // listing and any cursor it held.
        let (paging_signal, paging_album, paging_action, paging_work) = (
            state.paging,
            state.album,
            state.action.clone(),
            state.work.clone(),
        );
        let paging_toggle = button(
            paging_label(paging),
            move |_state: &mut BrowsePanelState| {
                let next = match paging_signal.get_untracked() {
                    Paging::Offset => Paging::Cursor,
                    Paging::Cursor => Paging::Offset,
                };
                paging_signal.set(next);
                queue(
                    &paging_action,
                    &paging_work,
                    BrowseAction::Select(paging_album.get_untracked(), next),
                );
            },
        )
        .style(selector_style(paging == Paging::Cursor))
        .disabled(busy)
        .small();

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
                     as thumbnails. Load more fetches the next page. Cursor paging pages \
                     with a cursor instead of an offset, so a library change between pages \
                     neither repeats nor skips an asset; it has no total, and switching it \
                     reloads the first page. With limited access \
                     Manage selection reopens the system selection. Cells badge the \
                     content type (HEIC, RAW, ...) and mark favourites. Thumbnails share \
                     the cache's allocation without copying it, and each cell copies the \
                     pixels once to build its image; a cell that leaves the screen cancels \
                     its request, and the counters below show both. Tap a thumbnail to select it \
                     for the Originals panel.",
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
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(refresh_browse_button(state, busy))
                    .child(paging_toggle),
            )
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
                            listing_line(&listing)
                        })
                        .size(12.0)
                        .color(muted),
                    )
                    .child(SizedBox(Some(12.0), None))
                    .child(more_button(state, busy, next_page)),
            )
            .child(component(ThumbStatsLine))
            .child(text(message).size(11.0).color(muted))
    }
}

/// Shown for the access line before the first Refresh.
const UNREAD_ACCESS: &str = "Photo access: not read yet";

/// One list row: up to [`COLUMNS`] thumbnail cells with a gap between them.
fn thumb_row(
    assets: Vec<GridAsset>,
    selected: RwSignal<Option<AssetId>>,
) -> impl View<BrowsePanelState> {
    let mut cells = row();
    for (index, asset) in assets.iter().enumerate() {
        if index > 0 {
            cells = cells.child(SizedBox(Some(GAP), None));
        }
        cells = cells.child(component(ThumbCell {
            asset: asset.clone(),
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
    let (access, limited, album, paging) = (state.access, state.limited, state.album, state.paging);
    button("Refresh", move |_state: &mut BrowsePanelState| {
        let (line, is_limited) = read_access();
        access.set(line);
        limited.set(is_limited);
        queue(
            &action,
            &work,
            BrowseAction::Refresh(album.get_untracked(), paging.get_untracked()),
        );
    })
    .style(ButtonStyle::Secondary)
    .disabled(busy)
    .small()
}

fn manage_button(state: &BrowsePanelState, busy: bool) -> impl View<BrowsePanelState> {
    let (action, work, album) = (state.action.clone(), state.work.clone(), state.album);
    let paging = state.paging;
    button("Manage selection", move |_state: &mut BrowsePanelState| {
        queue(
            &action,
            &work,
            BrowseAction::Manage(album.get_untracked(), paging.get_untracked()),
        );
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
    let (action, work, paging) = (state.action.clone(), state.work.clone(), state.paging);
    button(label.to_string(), move |_state: &mut BrowsePanelState| {
        queue(
            &action,
            &work,
            BrowseAction::Select(target.clone(), paging.get_untracked()),
        );
    })
    .style(selector_style(selected))
    .disabled(busy)
    .small()
}

/// Load more: queues `next` (see [`more_action`]); disabled while busy or without a next page.
fn more_button(
    state: &BrowsePanelState,
    busy: bool,
    next: Option<BrowseAction>,
) -> impl View<BrowsePanelState> {
    let (action, work) = (state.action.clone(), state.work.clone());
    let disabled = busy || next.is_none();
    button("Load more", move |_state: &mut BrowsePanelState| {
        if let Some(next) = next.clone() {
            queue(&action, &work, next);
        }
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

/// The cap of the capped decode button, in pixels.
const DECODE_CAP: u32 = 2048;

/// The cap of the full-size decode button, in pixels. There is no unbounded mode, so this is the
/// largest side a result may have.
const DECODE_FULL_CAP: u32 = 16384;

/// A pending decode: the asset, the cap and the token **Cancel decode** cancels.
struct DecodeAction {
    id: AssetId,
    max_dimension: u32,
    export: ExportOptions,
    cancel: CancelToken,
}

/// A finished decode as the panel shows it. The image source is made once, in the background
/// step, and cloned (a pointer copy) by every build.
#[derive(Clone)]
struct DecodeShown {
    source: ImageSource,
    lines: Vec<String>,
}

/// A decode's outcome: the shown result, or the typed error as a line.
type DecodeOutcome = Result<DecodeShown, String>;

/// The colour space as a label.
fn color_space_text(space: ImageColorSpace) -> &'static str {
    match space {
        ImageColorSpace::Srgb => "sRGB",
        ImageColorSpace::DisplayP3 => "Display P3",
        _ => "other",
    }
}

/// The source and output sizes against the cap.
fn decode_size_line(output: (u32, u32), source: (u32, u32), cap: u32) -> String {
    let scaled = if output == source {
        "not downscaled"
    } else {
        "downscaled"
    };
    format!(
        "Source {}x{}, output {}x{} (cap {cap}, {scaled})",
        source.0, source.1, output.0, output.1
    )
}

/// The EXIF fields the file carried as one line, or a note that it had none.
fn exif_summary(exif: Option<&ExifData>) -> String {
    let Some(exif) = exif else {
        return "EXIF: none".to_string();
    };
    let mut parts: Vec<String> = Vec::new();
    let camera = [exif.camera_make.as_deref(), exif.camera_model.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if !camera.is_empty() {
        parts.push(camera);
    }
    if let Some(lens) = &exif.lens {
        parts.push(lens.clone());
    }
    if let Some(time) = &exif.capture_time {
        parts.push(time.clone());
    }
    if let Some(iso) = exif.iso {
        parts.push(format!("ISO {iso}"));
    }
    if let Some(seconds) = exif.exposure_seconds {
        parts.push(format!("{seconds:.4}s"));
    }
    if let Some(aperture) = exif.aperture {
        parts.push(format!("f/{aperture:.1}"));
    }
    if let Some(focal) = exif.focal_length_mm {
        parts.push(format!("{focal:.1}mm"));
    }
    if let (Some(lat), Some(lon)) = (exif.gps_latitude, exif.gps_longitude) {
        parts.push(format!("GPS {lat:.4}, {lon:.4}"));
    }
    if parts.is_empty() {
        "EXIF: present, no summary fields".to_string()
    } else {
        format!("EXIF: {}", parts.join(" | "))
    }
}

/// A failed decode as one line: the display text and the variant.
fn decode_error_text(err: &PhotoError) -> String {
    match err {
        PhotoError::Cancelled => "Cancelled".to_string(),
        other => format!("decode error: {other} ({other:?})"),
    }
}

/// Runs one decode (call inside `spawn_blocking`). `None` is the mount-time fetch: nothing asked.
fn run_decode(action: Option<DecodeAction>) -> Option<DecodeOutcome> {
    let action = action?;
    let options = DecodeOptions::new(action.max_dimension).cancel(action.cancel);
    Some(
        match PhotoLibrary::decode_asset(&action.id, &options, &action.export) {
            Ok(image) => {
                let lines = vec![
                    decode_size_line(
                        (image.width, image.height),
                        (image.source_width, image.source_height),
                        action.max_dimension,
                    ),
                    format!("Colour space: {}", color_space_text(image.color_space)),
                    exif_summary(image.exif.as_ref()),
                ];
                Ok(DecodeShown {
                    source: ImageSource::from_rgba8(image.rgba, image.width, image.height),
                    lines,
                })
            }
            Err(err) => Err(decode_error_text(&err)),
        },
    )
}

/// A pending metadata read: the asset and whether an iCloud-only original may be downloaded.
struct MetadataAction {
    id: AssetId,
    allow_network: bool,
    /// The token **Cancel metadata** cancels; fresh for every read.
    cancel: CancelToken,
}

/// A metadata result as one line: the EXIF summary, a cancelled read as `Cancelled`, or the typed
/// error (a `NetworkRequired` shows as such, with its variant).
fn metadata_text(result: Result<ExifData, PhotoError>) -> String {
    match result {
        Ok(exif) => exif_summary(Some(&exif)),
        Err(PhotoError::Cancelled) => "Cancelled".to_string(),
        Err(err) => format!("metadata error: {err} ({err:?})"),
    }
}

/// Reads one asset's metadata (call inside `spawn_blocking`). `None` is the mount-time fetch.
fn run_metadata(action: Option<MetadataAction>) -> Option<String> {
    let action = action?;
    let options = MetadataOptions::new()
        .allow_network(action.allow_network)
        .cancel(action.cancel);
    Some(metadata_text(PhotoLibrary::asset_metadata(
        &action.id, &options,
    )))
}

/// The decode edge of the decoded image's display box, in logical points.
const DECODE_BOX: f64 = 240.0;

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
    /// The pending decode; `None` resolves to nothing.
    decode_action: Arc<Mutex<Option<DecodeAction>>>,
    /// The latest decode; its value carries the result, nothing sets a signal in the future.
    decode: UseTask<Option<DecodeOutcome>>,
    /// The running decode's token; Cancel decode takes it, each press replaces it.
    decode_token: Arc<Mutex<Option<CancelToken>>>,
    /// Whether the metadata read may download an iCloud-only original (default off).
    metadata_network: RwSignal<bool>,
    /// The pending metadata read; `None` resolves to nothing.
    metadata_action: Arc<Mutex<Option<MetadataAction>>>,
    /// The running metadata read's token; Cancel metadata takes it, each Metadata press replaces it.
    metadata_token: Arc<Mutex<Option<CancelToken>>>,
    /// The latest metadata read; its value is the summary or error line.
    metadata: UseTask<Option<String>>,
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
        let decode_action: Arc<Mutex<Option<DecodeAction>>> = Arc::new(Mutex::new(None));
        let decode_cell = decode_action.clone();
        let decode = use_task(move || {
            let requested = decode_cell.lock().expect("decode cell poisoned").take();
            async move {
                spawn_blocking(move || run_decode(requested))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))
            }
        });
        let metadata_action: Arc<Mutex<Option<MetadataAction>>> = Arc::new(Mutex::new(None));
        let metadata_cell = metadata_action.clone();
        let metadata = use_task(move || {
            let requested = lock_cell(&metadata_cell).take();
            async move {
                spawn_blocking(move || run_metadata(requested))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))
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
            decode_action,
            decode,
            decode_token: Arc::new(Mutex::new(None)),
            metadata_network: RwSignal::new(false),
            metadata_action,
            metadata_token: Arc::new(Mutex::new(None)),
            metadata,
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
        let decoded = state.decode.signal().get();
        let decoding = matches!(decoded, AsyncValue::Loading(_));
        let metadata_value = state.metadata.signal().get();
        let metadata_busy = matches!(metadata_value, AsyncValue::Loading(_));
        let metadata_network_on = state.metadata_network.get();

        let mut versions = wrap().spacing(8.0).run_spacing(8.0);
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

        let mut decode_body = column();
        match &decoded {
            AsyncValue::Loading(_) => {
                decode_body = decode_body.child(text("Decoding\u{2026}").size(12.0).color(muted));
            }
            AsyncValue::Ready(Some(Ok(shown))) => {
                for line in &shown.lines {
                    decode_body = decode_body.child(text(line.clone()).size(11.0));
                }
                decode_body = decode_body.child(SizedBox(None, Some(4.0))).child(
                    SizedBox(Some(DECODE_BOX), Some(DECODE_BOX))
                        .child(Image(shown.source.clone()).fit(ImageFit::Contain)),
                );
            }
            AsyncValue::Ready(Some(Err(line))) => {
                decode_body = decode_body.child(text(line.clone()).size(11.0).color(muted));
            }
            AsyncValue::Error(err) => {
                decode_body = decode_body.child(text(err.to_string()).size(11.0).color(muted));
            }
            _ => {}
        }
        let decode_target = target.as_ref().map(|(id, _)| id.clone());
        let metadata_target = decode_target.clone();
        let metadata_line = match &metadata_value {
            AsyncValue::Loading(_) => "Reading metadata\u{2026}".to_string(),
            AsyncValue::Ready(Some(line)) => line.clone(),
            AsyncValue::Error(err) => err.to_string(),
            _ => String::new(),
        };
        let metadata_network_signal = state.metadata_network;
        let metadata_network_toggle = button(
            if metadata_network_on {
                "Metadata network: allowed"
            } else {
                "Metadata network: off"
            },
            move |_state: &mut OriginalsPanelState| {
                metadata_network_signal.set(!metadata_network_signal.get_untracked());
            },
        )
        .style(selector_style(metadata_network_on))
        .disabled(metadata_busy)
        .small();

        column()
            .child(text("Originals").size(13.0).color(accent))
            .child(
                text(
                    "Export copies the selected asset's original file (or its edited \
                     rendition) into the app cache off the UI thread; an iCloud-only \
                     original downloads with progress when the network is allowed, and \
                     Cancel stops it. The result line shows the file's name, mime, size and \
                     first bytes. Save copy back files the exported copy into the library. \
                     Metadata reads the asset's EXIF without exporting it.",
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
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(export_button(state, target.map(|(id, _)| id), busy))
                    .child(originals_cancel_button(state, busy)),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(body)
            .child(text(result).size(11.0))
            .child(SizedBox(None, Some(8.0)))
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(save_button(state, "Save copy back", true, can_save))
                    .child(save_button(state, "Save (no album)", false, can_save)),
            )
            .child(text(saved).size(11.0).color(muted))
            .child(SizedBox(None, Some(8.0)))
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(decode_button(
                        state,
                        "Decode (2048)",
                        DECODE_CAP,
                        decode_target.clone(),
                        busy || decoding,
                    ))
                    .child(decode_button(
                        state,
                        "Decode (full, cap 16384)",
                        DECODE_FULL_CAP,
                        decode_target,
                        busy || decoding,
                    ))
                    .child(decode_cancel_button(state, decoding)),
            )
            .child(SizedBox(None, Some(4.0)))
            .child(decode_body)
            .child(SizedBox(None, Some(8.0)))
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(metadata_button(state, metadata_target, metadata_busy))
                    .child(metadata_cancel_button(state, metadata_busy))
                    .child(metadata_network_toggle),
            )
            .child(SizedBox(None, Some(4.0)))
            .child(text(metadata_line).size(11.0).color(muted))
    }
}

fn decode_button(
    state: &OriginalsPanelState,
    label: &'static str,
    max_dimension: u32,
    target: Option<AssetId>,
    busy: bool,
) -> impl View<OriginalsPanelState> {
    let (action, decode, token) = (
        state.decode_action.clone(),
        state.decode.clone(),
        state.decode_token.clone(),
    );
    let (version, network) = (state.version, state.network);
    let disabled = busy || target.is_none();
    button(label, move |_state: &mut OriginalsPanelState| {
        let Some(id) = target.clone() else {
            return;
        };
        // A fresh token for every decode; the export step reads the same one.
        let cancel = CancelToken::new();
        *token.lock().expect("decode token cell poisoned") = Some(cancel.clone());
        let export = ExportOptions::new()
            .version(VERSIONS[version.get_untracked()].1)
            .allow_network(network.get_untracked())
            .cancel(cancel.clone());
        *action.lock().expect("decode cell poisoned") = Some(DecodeAction {
            id,
            max_dimension,
            export,
            cancel,
        });
        decode.restart();
    })
    .style(ButtonStyle::Secondary)
    .disabled(disabled)
    .small()
}

fn metadata_button(
    state: &OriginalsPanelState,
    target: Option<AssetId>,
    busy: bool,
) -> impl View<OriginalsPanelState> {
    let (action, metadata, network, token) = (
        state.metadata_action.clone(),
        state.metadata.clone(),
        state.metadata_network,
        state.metadata_token.clone(),
    );
    let disabled = busy || target.is_none();
    button("Metadata", move |_state: &mut OriginalsPanelState| {
        let Some(id) = target.clone() else {
            return;
        };
        // A fresh token for every read; the read and Cancel metadata share it.
        let cancel = CancelToken::new();
        *lock_cell(&token) = Some(cancel.clone());
        *lock_cell(&action) = Some(MetadataAction {
            id,
            allow_network: network.get_untracked(),
            cancel,
        });
        metadata.restart();
    })
    .style(ButtonStyle::Secondary)
    .disabled(disabled)
    .small()
}

fn metadata_cancel_button(
    state: &OriginalsPanelState,
    busy: bool,
) -> impl View<OriginalsPanelState> {
    let token = state.metadata_token.clone();
    button(
        "Cancel metadata",
        move |_state: &mut OriginalsPanelState| {
            let current = lock_cell(&token).clone();
            if let Some(current) = current {
                current.cancel();
            }
        },
    )
    .style(ButtonStyle::Secondary)
    .disabled(!busy)
    .small()
}

fn decode_cancel_button(
    state: &OriginalsPanelState,
    decoding: bool,
) -> impl View<OriginalsPanelState> {
    let token = state.decode_token.clone();
    button("Cancel decode", move |_state: &mut OriginalsPanelState| {
        let current = token.lock().expect("decode token cell poisoned").clone();
        if let Some(current) = current {
            current.cancel();
        }
    })
    .style(ButtonStyle::Secondary)
    .disabled(!decoding)
    .small()
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

/// The shared-preferences key holding the Changes panel's change token, as lowercase hex.
const CHANGE_TOKEN_KEY: &str = "playground.change_token";

/// How often the watch task copies the wake-up count into its signal.
const WAKE_POLL: Duration = Duration::from_millis(250);

/// Lowercase hex of a token's bytes, the form the token is stored in.
fn token_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The bytes of a stored hex token; `None` when the text is not hex pairs.
fn token_from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

/// The stored token, if one is stored and readable.
fn stored_token(prefs: &SharedPreferences) -> Option<ChangeToken> {
    prefs
        .get_string(CHANGE_TOKEN_KEY)
        .and_then(|text| token_from_hex(&text))
        .map(ChangeToken::from_bytes)
}

/// Stores `token` for the next Check, and the next launch.
fn store_token(prefs: &SharedPreferences, token: &ChangeToken) -> Result<(), String> {
    prefs
        .set_string(CHANGE_TOKEN_KEY, token_to_hex(token.as_bytes()))
        .map_err(|err| format!("could not store the token: {err}"))
}

/// The stored-token line: whether one is stored and its byte length, never its bytes.
fn token_line(stored: Option<usize>) -> String {
    match stored {
        Some(len) => format!("Stored token: {len} bytes"),
        None => "No token stored".to_string(),
    }
}

/// A change report as one line, and the token it answers with.
fn changes_summary(changes: &LibraryChanges) -> (String, Option<&ChangeToken>) {
    match changes {
        LibraryChanges::Delta {
            inserted,
            updated,
            deleted,
            next,
        } => (
            format!(
                "Delta: +{} ~{} -{}",
                inserted.len(),
                updated.len(),
                deleted.len()
            ),
            Some(next),
        ),
        LibraryChanges::FullRescan { next } => ("Full rescan".to_string(), Some(next)),
        _ => ("Unrecognised change report".to_string(), None),
    }
}

/// What a Changes press asks the background step to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangesAction {
    /// The mount-time step: read the stored token back.
    Load,
    /// Store the library's current token.
    Save,
    /// Ask what changed since the stored token, then store the token the answer carries, and
    /// fetch the inserted and updated assets by id.
    Check,
    /// Time a full details listing of the library, for comparison with the by-id fetch.
    TimeListing,
}

/// One Changes step's result, carried in the task's value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangesOutcome {
    /// The stored token's byte length after the step; `None` when none is stored.
    stored: Option<usize>,
    /// The step's result line; empty for the mount-time load.
    line: String,
    /// Per-id lines of a Check's by-id fetch (the timing line first); empty otherwise.
    detail: Vec<String>,
}

/// The by-id fetch of a delta's inserted and updated ids: a timing line, then one line per id
/// (the file name and MIME type, or `not visible`). `fetch` is `assets_by_id` with details (call
/// inside `spawn_blocking`; tests pass stand-ins).
fn fetch_lines(
    ids: &[AssetId],
    fetch: impl FnOnce(&[AssetId]) -> Result<Vec<Option<Asset>>, PhotoError>,
) -> Vec<String> {
    if ids.is_empty() {
        return Vec::new();
    }
    let started = Instant::now();
    let result = fetch(ids);
    let elapsed = started.elapsed().as_millis();
    match result {
        Ok(found) => {
            let mut lines = vec![format!("Fetched {} id(s) by id in {elapsed} ms", ids.len())];
            lines.extend(ids.iter().enumerate().map(|(at, id)| {
                match found.get(at).and_then(Option::as_ref) {
                    Some(asset) => format!(
                        "{}: {} | {}",
                        id.as_str(),
                        asset.file_name.as_deref().unwrap_or("no name"),
                        asset.mime_type().as_deref().unwrap_or("no mime type"),
                    ),
                    None => format!("{}: not visible", id.as_str()),
                }
            }));
            lines
        }
        Err(err) => vec![format!(
            "Fetching {} id(s) by id failed after {elapsed} ms: error: {err}",
            ids.len()
        )],
    }
}

/// Lists the whole library with details, page by page, and reports the asset count and the
/// elapsed milliseconds (call inside `spawn_blocking`).
fn time_full_listing() -> String {
    let started = Instant::now();
    let mut count = 0u64;
    loop {
        let query = AssetQuery::new().limit(500).offset(count).with_details();
        match PhotoLibrary::assets(&query) {
            Ok(page) => {
                count += page.items.len() as u64;
                if page.items.is_empty() || count >= page.total {
                    break;
                }
            }
            Err(err) => return format!("Full listing: error: {err}"),
        }
    }
    format!(
        "Full listing: {count} asset(s) with details in {} ms",
        started.elapsed().as_millis()
    )
}

/// Runs `action` against `prefs`, with `current`, `since` and `fetch` as the token, change and
/// by-id calls (call inside `spawn_blocking` with the plugin's calls; tests pass stand-ins).
#[cfg(test)]
fn changes_step(
    prefs: &SharedPreferences,
    action: ChangesAction,
    current: impl FnOnce() -> Result<ChangeToken, PhotoError>,
    since: impl FnOnce(&ChangeToken) -> Result<LibraryChanges, PhotoError>,
) -> ChangesOutcome {
    changes_step_fetching(prefs, action, current, since, |_| {
        Err(PhotoError::Unsupported)
    })
}

/// [`changes_step`] with the by-id call supplied.
fn changes_step_fetching(
    prefs: &SharedPreferences,
    action: ChangesAction,
    current: impl FnOnce() -> Result<ChangeToken, PhotoError>,
    since: impl FnOnce(&ChangeToken) -> Result<LibraryChanges, PhotoError>,
    fetch: impl FnOnce(&[AssetId]) -> Result<Vec<Option<Asset>>, PhotoError>,
) -> ChangesOutcome {
    let mut detail = Vec::new();
    let line = match action {
        // The listing is timed by `run_changes`, which has no token work to do.
        ChangesAction::Load | ChangesAction::TimeListing => String::new(),
        ChangesAction::Save => match current() {
            Ok(token) => match store_token(prefs, &token) {
                Ok(()) => "Saved the current token.".to_string(),
                Err(text) => text,
            },
            Err(err) => format!("error: {err}"),
        },
        ChangesAction::Check => match stored_token(prefs) {
            None => "No token stored: press Save token first.".to_string(),
            Some(token) => match since(&token) {
                Ok(changes) => {
                    let (summary, next) = changes_summary(&changes);
                    if let LibraryChanges::Delta {
                        inserted, updated, ..
                    } = &changes
                    {
                        let ids: Vec<AssetId> =
                            inserted.iter().chain(updated.iter()).cloned().collect();
                        detail = fetch_lines(&ids, fetch);
                    }
                    match next.map(|next| store_token(prefs, next)) {
                        Some(Err(text)) => format!("{summary}, but {text}"),
                        _ => summary,
                    }
                }
                Err(err) => format!("error: {err}"),
            },
        },
    };
    ChangesOutcome {
        stored: stored_token(prefs).map(|token| token.as_bytes().len()),
        line,
        detail,
    }
}

/// Runs one Changes action against the app's preferences (call inside `spawn_blocking`).
fn run_changes(action: ChangesAction) -> ChangesOutcome {
    match SharedPreferences::standard() {
        Ok(prefs) => {
            let mut outcome = changes_step_fetching(
                &prefs,
                action,
                PhotoLibrary::current_change_token,
                PhotoLibrary::changes_since,
                |ids| PhotoLibrary::assets_by_id(ids, true),
            );
            if action == ChangesAction::TimeListing {
                outcome.line = time_full_listing();
            }
            outcome
        }
        Err(err) => ChangesOutcome {
            stored: None,
            line: format!("preferences unavailable: {err}"),
            detail: Vec::new(),
        },
    }
}

/// The watch line: the wake-up count while watching, the last count after Stop, or a subscribe
/// error.
fn watch_line(watching: bool, wakes: u64, error: Option<&str>) -> String {
    match (watching, error) {
        (true, _) => format!("Watching: {wakes} wake-up(s)"),
        (false, Some(text)) => text.to_string(),
        (false, None) => format!("Not watching ({wakes} wake-up(s) counted)"),
    }
}

/// The Changes panel's library-change watch, held in [`PlaygroundState`] so a watch outlives a
/// switch to another section. Clones share one subscription and one count.
///
/// The subscription's callback runs on an arbitrary thread, so it only bumps `wakes`; the
/// panel's watch task copies the count into a signal.
#[derive(Clone, Default)]
pub struct LibraryWatch {
    subscription: Arc<Mutex<Option<ChangeSubscription>>>,
    wakes: Arc<AtomicU64>,
}

impl LibraryWatch {
    /// Whether a subscription is live.
    fn is_active(&self) -> bool {
        lock_cell(&self.subscription).is_some()
    }

    /// The wake-ups counted since the live (or last) subscription started.
    fn wakes(&self) -> u64 {
        self.wakes.load(Ordering::Relaxed)
    }

    /// Subscribes, unless a subscription is already live, with the count restarted at zero (call
    /// inside `spawn_blocking`). A failure leaves nothing subscribed.
    fn start(&self) -> Result<(), PhotoError> {
        let mut held = lock_cell(&self.subscription);
        if held.is_some() {
            return Ok(());
        }
        self.wakes.store(0, Ordering::Relaxed);
        let wakes = Arc::clone(&self.wakes);
        *held = Some(PhotoLibrary::on_library_changed(move || {
            wakes.fetch_add(1, Ordering::Relaxed);
        })?);
        Ok(())
    }

    /// Drops the subscription, which unregisters the callback (and stops the platform observer
    /// when it was the last one). The drop runs outside the cell's lock.
    fn stop(&self) {
        let ended = lock_cell(&self.subscription).take();
        drop(ended);
    }
}

struct ChangesPanel {
    /// The app-held watch; see [`LibraryWatch`].
    watch: LibraryWatch,
}

struct ChangesPanelState {
    /// The pending action; the mount-time run loads, and every run leaves `Load` behind.
    action: Arc<Mutex<ChangesAction>>,
    /// The latest step's result; a press restarts it.
    outcome: UseTask<ChangesOutcome>,
    watch: LibraryWatch,
    /// Whether the next watch run subscribes; the mount-time run only resumes a live watch.
    start: Arc<Mutex<bool>>,
    /// Loading while a watch lasts; ready with the subscribe error text, if any, once it ends.
    watching: UseTask<Option<String>>,
    /// The wake-up count, copied from the watch by the watch task.
    wakes: RwSignal<u64>,
}

impl Component for ChangesPanel {
    type State = ChangesPanelState;

    fn init(&self) -> ChangesPanelState {
        let action = Arc::new(Mutex::new(ChangesAction::Load));
        let cell = action.clone();
        let outcome = use_task(move || {
            let requested = std::mem::replace(&mut *lock_cell(&cell), ChangesAction::Load);
            async move {
                spawn_blocking(move || run_changes(requested))
                    .await
                    .map_err(|err| JoinFailure(err.to_string()))
            }
        });

        let start = Arc::new(Mutex::new(false));
        let wakes = RwSignal::new(self.watch.wakes());
        let (start_cell, task_watch) = (start.clone(), self.watch.clone());
        let watching = use_task(move || {
            let subscribe = std::mem::take(&mut *lock_cell(&start_cell));
            let watch = task_watch.clone();
            async move {
                if subscribe {
                    let starting = watch.clone();
                    let started = spawn_blocking(move || starting.start())
                        .await
                        .map_err(|err| JoinFailure(err.to_string()))?;
                    if let Err(err) = started {
                        return Ok(Some(format!("error: {err}")));
                    }
                }
                // The callback only bumps the shared count: copy it into the signal while the
                // watch lasts, and stop once the panel is gone.
                while watch.is_active() {
                    let Some(current) = wakes.try_get_untracked() else {
                        return Ok(None);
                    };
                    // `try_update`: the panel can be torn down between the read above and this
                    // write, and `set` panics on a disposed signal.
                    let latest = watch.wakes();
                    if latest != current && wakes.try_update(|count| *count = latest).is_none() {
                        return Ok(None);
                    }
                    let _ = spawn_blocking(move || std::thread::sleep(WAKE_POLL)).await;
                }
                let latest = watch.wakes();
                if wakes
                    .try_get_untracked()
                    .is_some_and(|current| current != latest)
                {
                    let _ = wakes.try_update(|count| *count = latest);
                }
                Ok::<Option<String>, JoinFailure>(None)
            }
        });

        ChangesPanelState {
            action,
            outcome,
            watch: self.watch.clone(),
            start,
            watching,
            wakes,
        }
    }

    fn build(&self, state: &mut ChangesPanelState) -> impl View<ChangesPanelState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let outcome = state.outcome.signal().get();
        let busy = matches!(outcome, AsyncValue::Loading(_));
        let (token_text, result_text, detail_lines) = match &outcome {
            AsyncValue::Ready(done) => (
                token_line(done.stored),
                done.line.clone(),
                done.detail.clone(),
            ),
            AsyncValue::Loading(Some(previous)) => (
                token_line(previous.stored),
                "Working\u{2026}".to_string(),
                Vec::new(),
            ),
            AsyncValue::Error(err) => (
                "Stored token: unknown".to_string(),
                err.to_string(),
                Vec::new(),
            ),
            _ => (
                "Reading the stored token\u{2026}".to_string(),
                String::new(),
                Vec::new(),
            ),
        };

        let watch_value = state.watching.signal().get();
        let watching = matches!(watch_value, AsyncValue::Loading(_));
        let watch_error = match &watch_value {
            AsyncValue::Ready(Some(text)) => Some(text.clone()),
            AsyncValue::Error(err) => Some(err.to_string()),
            _ => None,
        };
        let watch_text = watch_line(watching, state.wakes.get(), watch_error.as_deref());

        let press = |label: &'static str, next: ChangesAction| {
            let (cell, task) = (state.action.clone(), state.outcome.clone());
            button(label, move |_state: &mut ChangesPanelState| {
                *lock_cell(&cell) = next;
                task.restart();
            })
            .style(ButtonStyle::Secondary)
            .disabled(busy)
            .small()
        };
        let (start, watch_task) = (state.start.clone(), state.watching.clone());
        let watch_button = button("Watch", move |_state: &mut ChangesPanelState| {
            *lock_cell(&start) = true;
            watch_task.restart();
        })
        .disabled(watching)
        .small();
        let stop_watch = state.watch.clone();
        let stop_button = button("Stop", move |_state: &mut ChangesPanelState| {
            stop_watch.stop();
        })
        .style(ButtonStyle::Secondary)
        .disabled(!watching)
        .small();

        column()
            .child(text("Changes").size(13.0).color(accent))
            .child(
                text(
                    "Save token stores the library's current change token in shared \
                     preferences, so the next launch reads it back. Check asks what changed \
                     since the stored token (a delta of inserted, updated and deleted assets, \
                     or a full rescan), stores the token it answers with, and fetches the inserted and \
                     updated assets by id, showing each file name and MIME type (or not \
                     visible) with the call's time. Time full listing lists the whole library \
                     with details for comparison. Watch counts the \
                     library's change wake-ups until Stop.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(text(token_text).size(12.0))
            .child(SizedBox(None, Some(4.0)))
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(press("Save token", ChangesAction::Save))
                    .child(press("Check", ChangesAction::Check))
                    .child(press("Time full listing", ChangesAction::TimeListing)),
            )
            .child(text(result_text).size(12.0).color(muted))
            .children(views::<ChangesPanelState, _>(
                detail_lines
                    .into_iter()
                    .map(|line| text(line).size(11.0).color(muted)),
            ))
            .child(SizedBox(None, Some(8.0)))
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(watch_button)
                    .child(stop_button),
            )
            .child(text(watch_text).size(12.0).color(muted))
    }
}

/// The shared-preferences key holding the picked folder's grant token.
pub(super) const FOLDER_TOKEN_KEY: &str = "playground.folder_token";

/// The shared-preferences key holding the first scanned file's relative path, beside the token.
pub(super) const FOLDER_ENTRY_PATH_KEY: &str = "playground.folder_entry_path";

/// A relative path no folder holds, for the missing-path control.
const MISSING_ENTRY_PATH: &str = "frust-playground-missing/no-such-file.bin";

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
    /// Look up the stored first-file path through `FolderGrant::entry`.
    OpenStored,
    /// Read the stored path's first 16 bytes and show them as hex.
    SniffStored,
    /// Read the stored path with a 16-byte limit, to show the too-large report.
    OpenStoredSmall,
    /// Look up a path that cannot exist through `FolderGrant::entry`.
    OpenMissing,
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
                Ok(()) => format!("Restored {name}. Scan to list it."),
                Err(text) => format!("Restored {name}, but {text}."),
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
                Ok(()) => format!("Picked {name}. Scan to list it."),
                Err(text) => format!("Picked {name}, but {text}."),
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
    let _ = prefs.remove(FOLDER_ENTRY_PATH_KEY);
    match grant.map(Arc::try_unwrap) {
        None => FoldersStep::cleared("No folder to release."),
        Some(Ok(grant)) => {
            grant.release();
            FoldersStep::cleared("Released the folder and forgot its token.")
        }
        Some(Err(_)) => FoldersStep::cleared("Forgot the folder token (a scan still held it)."),
    }
}

/// Stores the first file of a first scan page, so a later launch can look it up by path.
fn remember_first_path(prefs: &SharedPreferences, step: &FoldersStep) {
    if let FoldersStep::Page {
        page, offset: 0, ..
    } = step
        && let Some(first) = page.items.first()
    {
        let _ = prefs.set_string(FOLDER_ENTRY_PATH_KEY, &first.entry.relative_path);
    }
}

/// The bytes a sniff reads from the start of the stored path.
const SNIFF_BYTES: u64 = 16;

/// The deliberately small `read_to_vec` limit of the over-limit control.
const SMALL_LIMIT_BYTES: u64 = 16;

/// How an entry found by path is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathRead {
    /// `read_to_vec` with this limit.
    Whole(u64),
    /// `read_prefix` of this many bytes.
    Prefix(u64),
}

/// Bytes as space-separated lowercase hex pairs.
fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A read failure as one line: a refused over-limit read shows its limit and the file's size.
fn read_error_text(err: &FileDialogError) -> String {
    match err {
        FileDialogError::TooLarge { limit, size } => format!(
            "too large: limit {limit} bytes, size {}",
            size.map_or_else(|| "unknown".to_string(), |size| format!("{size} bytes"))
        ),
        other => format!("{other:?}"),
    }
}

/// The result line of reading `entry` as `read` asks.
fn read_entry_line(entry: &FolderEntry, read: PathRead) -> String {
    match read {
        PathRead::Whole(limit) => match entry.read_to_vec(limit) {
            Ok(bytes) => format!("found {} {} bytes", entry.name, bytes.len()),
            Err(err) => read_error_text(&err),
        },
        PathRead::Prefix(count) => match entry.read_prefix(count) {
            Ok(bytes) => format!(
                "sniffed {} ({} bytes): {}",
                entry.name,
                bytes.len(),
                hex_bytes(&bytes)
            ),
            Err(err) => read_error_text(&err),
        },
    }
}

/// Looks `path` up in the held grant, restoring the stored one first when none is held
/// (call inside `spawn_blocking`): `found <name> <n> bytes`, `not found`, or the error's
/// Debug name.
fn open_path_step(
    prefs: &SharedPreferences,
    grants: &GrantCell,
    path: &str,
    read: PathRead,
) -> FoldersStep {
    let held = grants.lock().expect("grant cell poisoned").is_some();
    if !held {
        let restored = restore_step(prefs, grants);
        if grants.lock().expect("grant cell poisoned").is_none() {
            return restored;
        }
    }
    let grant = grants.lock().expect("grant cell poisoned").clone();
    let Some(grant) = grant else {
        return FoldersStep::message("Pick a folder first.");
    };
    FoldersStep::message(match grant.entry(path) {
        Ok(Some(entry)) => read_entry_line(&entry, read),
        Ok(None) => "not found".to_string(),
        Err(err) => format!("{err:?}"),
    })
}

/// The stored path's lookup, or a hint when nothing was scanned yet.
fn open_stored_step(prefs: &SharedPreferences, grants: &GrantCell, read: PathRead) -> FoldersStep {
    match prefs.get_string(FOLDER_ENTRY_PATH_KEY) {
        Some(path) => open_path_step(prefs, grants, &path, read),
        None => FoldersStep::message("No stored path. Scan a folder first."),
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
        FoldersAction::Scan { offset, recursive } => {
            let step = scan_step(grants, offset, recursive, cancel);
            remember_first_path(&prefs, &step);
            step
        }
        FoldersAction::Release => release_step(&prefs, grants),
        FoldersAction::OpenStored => {
            open_stored_step(&prefs, grants, PathRead::Whole(MAX_PREVIEW_BYTES))
        }
        FoldersAction::SniffStored => {
            open_stored_step(&prefs, grants, PathRead::Prefix(SNIFF_BYTES))
        }
        FoldersAction::OpenStoredSmall => {
            open_stored_step(&prefs, grants, PathRead::Whole(SMALL_LIMIT_BYTES))
        }
        FoldersAction::OpenMissing => open_path_step(
            &prefs,
            grants,
            MISSING_ENTRY_PATH,
            PathRead::Whole(MAX_PREVIEW_BYTES),
        ),
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
        FoldersAction::OpenStored
        | FoldersAction::SniffStored
        | FoldersAction::OpenStoredSmall
        | FoldersAction::OpenMissing => "Looking up the path\u{2026}",
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
                     decodes the first eight images and remembers the first file's path. Open stored \
                     path looks that path up in the folder; Open missing path looks up one that \
                     cannot exist. Release gives the folder up.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(
                wrap()
                    .spacing(8.0)
                    .run_spacing(8.0)
                    .child(press_button(
                        "Pick folder",
                        FoldersAction::Pick,
                        state,
                        busy,
                        false,
                    ))
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
                    .child(recursive_button(state, recursive))
                    .child(press_button(
                        "Release",
                        FoldersAction::Release,
                        state,
                        busy,
                        !holds,
                    ))
                    .child(press_button(
                        "Open stored path",
                        FoldersAction::OpenStored,
                        state,
                        busy,
                        false,
                    ))
                    .child(press_button(
                        "Sniff stored path",
                        FoldersAction::SniffStored,
                        state,
                        busy,
                        false,
                    ))
                    .child(press_button(
                        "Open stored path (16-byte limit)",
                        FoldersAction::OpenStoredSmall,
                        state,
                        busy,
                        false,
                    ))
                    .child(press_button(
                        "Open missing path",
                        FoldersAction::OpenMissing,
                        state,
                        busy,
                        false,
                    ))
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
    fn no_decode_resolves_to_nothing() {
        assert!(run_decode(None).is_none());
    }

    #[test]
    fn colour_space_labels() {
        assert_eq!(color_space_text(ImageColorSpace::Srgb), "sRGB");
        assert_eq!(color_space_text(ImageColorSpace::DisplayP3), "Display P3");
    }

    #[test]
    fn size_line_reports_both_sizes_and_the_cap() {
        assert_eq!(
            decode_size_line((2048, 1536), (4032, 3024), 2048),
            "Source 4032x3024, output 2048x1536 (cap 2048, downscaled)"
        );
        assert!(decode_size_line((10, 10), (10, 10), 2048).ends_with("not downscaled)"));
    }

    #[test]
    fn exif_summary_lists_present_fields() {
        assert_eq!(exif_summary(None), "EXIF: none");
        let mut exif = ExifData::default();
        assert_eq!(
            exif_summary(Some(&exif)),
            "EXIF: present, no summary fields"
        );
        exif.camera_make = Some("Apple".into());
        exif.camera_model = Some("iPhone".into());
        exif.iso = Some(100);
        exif.aperture = Some(1.8);
        assert_eq!(
            exif_summary(Some(&exif)),
            "EXIF: Apple iPhone | ISO 100 | f/1.8"
        );
    }

    #[test]
    fn decode_errors_are_typed_lines() {
        assert_eq!(decode_error_text(&PhotoError::Cancelled), "Cancelled");
        assert!(decode_error_text(&PhotoError::Unsupported).contains("Unsupported"));
    }

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

    fn ids(names: &[&str]) -> Vec<GridAsset> {
        names
            .iter()
            .map(|n| GridAsset {
                id: AssetId::from_raw(*n),
                badge: None,
                favorite: false,
            })
            .collect()
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
    fn both_platform_shapes_map_to_one_badge() {
        for (android, ios, badge) in [
            ("image/heic", "public.heic", "HEIC"),
            ("image/heif", "public.heif", "HEIC"),
            ("image/jpeg", "public.jpeg", "JPEG"),
            ("image/png", "public.png", "PNG"),
            ("image/x-adobe-dng", "com.adobe.raw-image", "RAW"),
            ("image/x-sony-arw", "public.camera-raw-image", "RAW"),
            ("video/quicktime", "com.apple.quicktime-movie", "MOV"),
            ("video/mp4", "public.mpeg-4", "MP4"),
        ] {
            assert_eq!(type_badge(Some(android)), Some(badge), "{android}");
            assert_eq!(type_badge(Some(ios)), Some(badge), "{ios}");
        }
        assert_eq!(type_badge(Some("IMAGE/HEIC")), Some("HEIC"));
        assert_eq!(type_badge(Some("application/x-unknown")), None);
        assert_eq!(type_badge(None), None);
    }

    #[test]
    fn details_fill_the_badge_and_the_favourite_mark() {
        let cell = GridAsset::new(AssetId::from_raw("a"), Some("public.heic"), Some(true));
        assert_eq!((cell.badge, cell.favorite), (Some("HEIC"), true));
        let cell = GridAsset::new(AssetId::from_raw("a"), None, None);
        assert_eq!((cell.badge, cell.favorite), (None, false));
        let cell = GridAsset::new(AssetId::from_raw("a"), Some("image/png"), Some(false));
        assert_eq!((cell.badge, cell.favorite), (Some("PNG"), false));
    }

    #[test]
    fn browse_queries_ask_for_details() {
        assert!(browse_query(&None, 0).details);
    }

    fn pixels() -> Arc<Thumbnail> {
        Arc::new(Thumbnail::new(1, 1, vec![0; 4]))
    }

    #[test]
    fn the_same_allocation_served_again_is_a_cache_hit() {
        let id = AssetId::from_raw("a");
        let mut seen = HashMap::new();
        let first = pixels();
        assert_eq!(note_served(&mut seen, &id, &first), ThumbServe::Loaded);
        assert_eq!(
            note_served(&mut seen, &id, &Arc::clone(&first)),
            ThumbServe::Cached
        );
        // A different allocation for the asset is a fresh load.
        assert_eq!(note_served(&mut seen, &id, &pixels()), ThumbServe::Loaded);
    }

    #[test]
    fn a_dropped_allocation_is_not_a_hit() {
        let id = AssetId::from_raw("a");
        let mut seen = HashMap::new();
        let first = pixels();
        note_served(&mut seen, &id, &first);
        drop(first);
        assert_eq!(note_served(&mut seen, &id, &pixels()), ThumbServe::Loaded);
    }

    #[test]
    fn counters_count_each_outcome_and_read_as_a_line() {
        let stats = ThumbStats {
            cached: AtomicU64::new(0),
            loaded: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        };
        for serve in [
            ThumbServe::Cached,
            ThumbServe::Cached,
            ThumbServe::Loaded,
            failure_serve(&PhotoError::Cancelled),
            failure_serve(&PhotoError::Busy),
        ] {
            stats.record(serve);
        }
        let counts = stats.snapshot();
        assert_eq!(
            counts,
            ThumbCounts {
                cached: 2,
                loaded: 1,
                cancelled: 1,
                failed: 1
            }
        );
        assert_eq!(
            counts.line(),
            "Thumbnails: 2 from cache (lower bound), 1 loaded, 1 cancelled, 1 failed"
        );
    }

    #[test]
    fn the_cache_counter_is_labelled_a_lower_bound() {
        let counts = ThumbCounts {
            cached: 3,
            ..ThumbCounts::default()
        };
        assert!(
            counts.line().contains("3 from cache (lower bound)"),
            "{}",
            counts.line()
        );
    }

    #[test]
    fn metadata_shows_the_summary_or_the_typed_error() {
        let mut exif = ExifData::default();
        exif.iso = Some(200);
        assert_eq!(metadata_text(Ok(exif)), "EXIF: ISO 200");
        let line = metadata_text(Err(PhotoError::NetworkRequired));
        assert!(line.starts_with("metadata error: "), "{line}");
        assert!(line.contains("NetworkRequired"), "{line}");
        assert_eq!(metadata_text(Err(PhotoError::Cancelled)), "Cancelled");
        assert!(run_metadata(None).is_none());
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
            message_of(&step).starts_with("Restored "),
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
    fn a_scan_remembers_the_first_path_and_open_stored_finds_it() {
        let dir = Scratch::new("open-stored");
        std::fs::write(dir.0.join("a.png"), b"xyz").unwrap();
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        restore_step(&prefs, &grants);
        let step = scan_step(&grants, 0, false, &FolderCancel::new());
        remember_first_path(&prefs, &step);
        assert_eq!(
            prefs.get_string(FOLDER_ENTRY_PATH_KEY).as_deref(),
            Some("a.png")
        );
        // A fresh cell: the grant is restored from the token first.
        let step = open_stored_step(&prefs, &no_grants(), PathRead::Whole(MAX_PREVIEW_BYTES));
        assert_eq!(message_of(&step), "found a.png 3 bytes");
    }

    #[test]
    fn open_missing_says_not_found_and_open_stored_without_a_path_hints() {
        let dir = Scratch::new("open-missing");
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(FOLDER_TOKEN_KEY, dir.0.to_str().unwrap())
            .unwrap();
        let grants = no_grants();
        let step = open_path_step(&prefs, &grants, MISSING_ENTRY_PATH, PathRead::Whole(1));
        assert_eq!(message_of(&step), "not found");
        let step = open_stored_step(&prefs, &grants, PathRead::Whole(1));
        assert!(message_of(&step).starts_with("No stored path"));
        let step = open_path_step(&prefs, &grants, "../up", PathRead::Whole(1));
        assert!(message_of(&step).starts_with("Io"), "{}", message_of(&step));
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

    #[test]
    fn token_hex_round_trips_and_rejects_non_hex() {
        let bytes = vec![0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(token_to_hex(&bytes), "000fa5ff");
        assert_eq!(token_from_hex("000fa5ff"), Some(bytes));
        assert_eq!(
            token_from_hex("000FA5FF"),
            Some(vec![0x00, 0x0f, 0xa5, 0xff])
        );
        assert_eq!(token_from_hex(""), Some(Vec::new()));
        assert_eq!(token_from_hex("abc"), None);
        assert_eq!(token_from_hex("zz"), None);
        assert_eq!(token_from_hex("+f"), None);
        assert_eq!(token_from_hex("\u{e9}\u{e9}"), None);
    }

    #[test]
    fn token_line_shows_the_length_only() {
        assert_eq!(token_line(Some(32)), "Stored token: 32 bytes");
        assert_eq!(token_line(None), "No token stored");
    }

    fn token(bytes: &[u8]) -> ChangeToken {
        ChangeToken::from_bytes(bytes.to_vec())
    }

    #[test]
    fn change_summaries() {
        let delta = LibraryChanges::Delta {
            inserted: vec![AssetId::from_raw("a"), AssetId::from_raw("b")],
            updated: vec![AssetId::from_raw("c")],
            deleted: Vec::new(),
            next: token(&[1, 2]),
        };
        let (line, next) = changes_summary(&delta);
        assert_eq!(line, "Delta: +2 ~1 -0");
        assert_eq!(next, Some(&token(&[1, 2])));
        let rescan = LibraryChanges::FullRescan { next: token(&[3]) };
        let (line, next) = changes_summary(&rescan);
        assert_eq!(line, "Full rescan");
        assert_eq!(next, Some(&token(&[3])));
    }

    fn no_current() -> Result<ChangeToken, PhotoError> {
        panic!("the step must not read the current token")
    }

    fn no_since(_: &ChangeToken) -> Result<LibraryChanges, PhotoError> {
        panic!("the step must not ask for changes")
    }

    #[test]
    fn load_reads_back_the_stored_token_length() {
        let prefs = SharedPreferences::in_memory();
        let empty = changes_step(&prefs, ChangesAction::Load, no_current, no_since);
        assert_eq!(empty.stored, None);
        assert_eq!(empty.line, "");
        prefs
            .set_string(CHANGE_TOKEN_KEY, "0102030405")
            .expect("in-memory store");
        let stored = changes_step(&prefs, ChangesAction::Load, no_current, no_since);
        assert_eq!(stored.stored, Some(5));
    }

    #[test]
    fn save_stores_the_current_token_as_hex() {
        let prefs = SharedPreferences::in_memory();
        let saved = changes_step(
            &prefs,
            ChangesAction::Save,
            || Ok(token(&[0xab, 0xcd])),
            no_since,
        );
        assert_eq!(saved.stored, Some(2));
        assert_eq!(saved.line, "Saved the current token.");
        assert_eq!(prefs.get_string(CHANGE_TOKEN_KEY).as_deref(), Some("abcd"));
        assert_eq!(stored_token(&prefs), Some(token(&[0xab, 0xcd])));
    }

    #[test]
    fn a_failed_save_shows_the_error_and_keeps_the_stored_token() {
        let prefs = SharedPreferences::in_memory();
        store_token(&prefs, &token(&[7])).expect("in-memory store");
        let failed = changes_step(
            &prefs,
            ChangesAction::Save,
            || Err(PhotoError::PermissionDenied),
            no_since,
        );
        assert_eq!(failed.stored, Some(1));
        assert_eq!(
            failed.line,
            format!("error: {}", PhotoError::PermissionDenied)
        );
    }

    #[test]
    fn check_without_a_token_says_so() {
        let prefs = SharedPreferences::in_memory();
        let checked = changes_step(&prefs, ChangesAction::Check, no_current, no_since);
        assert_eq!(checked.stored, None);
        assert_eq!(checked.line, "No token stored: press Save token first.");
    }

    #[test]
    fn check_reports_the_delta_and_stores_next() {
        let prefs = SharedPreferences::in_memory();
        store_token(&prefs, &token(&[1])).expect("in-memory store");
        let checked = changes_step(&prefs, ChangesAction::Check, no_current, |stored| {
            assert_eq!(stored, &token(&[1]));
            Ok(LibraryChanges::Delta {
                inserted: vec![AssetId::from_raw("new")],
                updated: vec![AssetId::from_raw("edited")],
                deleted: vec![AssetId::from_raw("gone")],
                next: token(&[2, 2, 2]),
            })
        });
        assert_eq!(checked.line, "Delta: +1 ~1 -1");
        assert_eq!(checked.stored, Some(3));
        assert_eq!(stored_token(&prefs), Some(token(&[2, 2, 2])));
    }

    #[test]
    fn check_reports_a_full_rescan_and_stores_next() {
        let prefs = SharedPreferences::in_memory();
        store_token(&prefs, &token(&[1])).expect("in-memory store");
        let checked = changes_step(&prefs, ChangesAction::Check, no_current, |_| {
            Ok(LibraryChanges::FullRescan {
                next: token(&[9, 9]),
            })
        });
        assert_eq!(checked.line, "Full rescan");
        assert_eq!(stored_token(&prefs), Some(token(&[9, 9])));
    }

    #[test]
    fn a_failed_check_shows_the_error_and_keeps_the_token() {
        let prefs = SharedPreferences::in_memory();
        store_token(&prefs, &token(&[1])).expect("in-memory store");
        let checked = changes_step(&prefs, ChangesAction::Check, no_current, |_| {
            Err(PhotoError::Unsupported)
        });
        assert_eq!(checked.line, format!("error: {}", PhotoError::Unsupported));
        assert_eq!(stored_token(&prefs), Some(token(&[1])));
    }

    #[test]
    fn an_unreadable_stored_token_counts_as_none() {
        let prefs = SharedPreferences::in_memory();
        prefs
            .set_string(CHANGE_TOKEN_KEY, "not hex")
            .expect("in-memory store");
        assert_eq!(stored_token(&prefs), None);
    }

    #[test]
    fn check_fetches_inserted_and_updated_ids_by_id() {
        let prefs = SharedPreferences::in_memory();
        store_token(&prefs, &token(&[1])).expect("in-memory store");
        let checked = changes_step_fetching(
            &prefs,
            ChangesAction::Check,
            no_current,
            |_| {
                Ok(LibraryChanges::Delta {
                    inserted: vec![AssetId::from_raw("new")],
                    updated: vec![AssetId::from_raw("edited")],
                    deleted: Vec::new(),
                    next: token(&[2]),
                })
            },
            |ids| {
                assert_eq!(ids.len(), 2);
                Ok(vec![None, None])
            },
        );
        assert_eq!(checked.detail.len(), 3);
        assert!(checked.detail[0].starts_with("Fetched 2 id(s) by id in "));
        assert_eq!(checked.detail[1], "new: not visible");
        assert_eq!(checked.detail[2], "edited: not visible");
    }

    #[test]
    fn an_empty_delta_fetches_nothing() {
        let lines = fetch_lines(&[], |_| panic!("no ids, no call"));
        assert!(lines.is_empty());
    }

    #[test]
    fn hex_bytes_and_read_errors() {
        assert_eq!(hex_bytes(&[0x89, 0x50, 0x0a]), "89 50 0a");
        assert_eq!(hex_bytes(&[]), "");
        assert_eq!(
            read_error_text(&FileDialogError::TooLarge {
                limit: 16,
                size: Some(99)
            }),
            "too large: limit 16 bytes, size 99 bytes"
        );
        assert_eq!(
            read_error_text(&FileDialogError::TooLarge {
                limit: 16,
                size: None
            }),
            "too large: limit 16 bytes, size unknown"
        );
    }

    #[test]
    fn watch_lines() {
        assert_eq!(watch_line(true, 3, None), "Watching: 3 wake-up(s)");
        assert_eq!(
            watch_line(false, 3, None),
            "Not watching (3 wake-up(s) counted)"
        );
        assert_eq!(watch_line(false, 0, Some("error: x")), "error: x");
    }

    #[test]
    fn an_idle_watch_is_inactive_and_stop_is_a_no_op() {
        let watch = LibraryWatch::default();
        assert!(!watch.is_active());
        watch.stop();
        assert!(!watch.is_active());
        assert_eq!(watch.wakes(), 0);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    fn a_failed_watch_subscribes_nothing() {
        let watch = LibraryWatch::default();
        assert!(watch.start().is_err());
        assert!(!watch.is_active());
    }

    #[test]
    fn offset_listing_lines_and_more() {
        let listing = Listing {
            ids: Arc::new(ids(&["a", "b"])),
            total: Some(5),
            next: None,
        };
        assert_eq!(listing_line(&listing), "Showing 2 of 5 assets");
        let album = Some(AlbumId::from_raw("x"));
        assert!(matches!(
            more_action(&album, &listing),
            Some(BrowseAction::More(Some(_), 2))
        ));
        let done = Listing {
            total: Some(2),
            ..listing
        };
        assert!(more_action(&album, &done).is_none());
        assert_eq!(listing_line(&Listing::default()), "Showing 0 of 0 assets");
    }

    #[test]
    fn a_cursor_listing_has_no_total_and_no_more_without_next() {
        let listing = Listing {
            ids: Arc::new(ids(&["a", "b", "c"])),
            total: None,
            next: None,
        };
        assert_eq!(
            listing_line(&listing),
            "Showing 3 assets (cursor paging, no total)"
        );
        assert!(more_action(&None, &listing).is_none());
    }

    #[test]
    fn an_offset_page_keeps_its_total_and_no_cursor() {
        let mut step = BrowseStep::default();
        let page = AssetPage {
            items: Vec::new(),
            total: 42,
        };
        record_page(&None, false, Ok(page), &mut step);
        match step.page {
            Some(PageUpdate::Replace(loaded)) => {
                assert_eq!(loaded.total, Some(42));
                assert!(loaded.next.is_none());
            }
            _ => panic!("expected a replacing page"),
        }
    }

    #[test]
    fn a_failed_cursor_page_keeps_the_listing_and_says_how_to_restart() {
        let mut step = BrowseStep::default();
        let err = PhotoError::InvalidRequest {
            reason: "the cursor's snapshot was evicted",
        };
        record_cursor_page(&None, true, Err(err.clone()), &mut step);
        assert!(step.page.is_none());
        assert!(step.album.is_none());
        assert_eq!(
            step.message,
            format!("error: {err}. Refresh starts again from the first page.")
        );
        assert_eq!(
            cursor_error_text(&PhotoError::Busy),
            format!("error: {}", PhotoError::Busy)
        );
    }

    #[test]
    fn paging_toggle_labels() {
        assert_eq!(paging_label(Paging::Offset), "Cursor paging: off");
        assert_eq!(paging_label(Paging::Cursor), "Cursor paging: on");
    }
}
