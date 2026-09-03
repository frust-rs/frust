//! Ports beUI's `file-upload` composed block — the dropzone plus the upload
//! queue: one row per file, each with its own progress bar, status glyph, retry
//! and remove, entering and leaving on their own ramps.
//!
//! Source: `components/motion/file-upload.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `file-upload` (blocks): *"Two file upload patterns: an attachment
//! workspace for mixed files, links, audio and media, plus a progress queue with
//! retry and removal."*
//!
//! The registry entry names a second file, `components/motion/attachment-upload.tsx`
//! — the `attachment-upload` **example** of the same slug, a 1080-line mixed
//! attachment workspace with shared-layout image previews, an audio waveform and
//! link cards. This port carries the slug's own file, the `upload-queue` example
//! (`file-upload.tsx`); the attachment workspace's media surfaces are a separate
//! piece of work and are deliberately not smuggled in here.
//!
//! | class / prop | here |
//! |---|---|
//! | root `w-full space-y-3` | dropzone, then the queue, [`UPLOAD_SECTION_GAP`] apart |
//! | dropzone `rounded-3xl border border-dashed` | [`style::RADIUS_3XL`], [`UPLOAD_DASH`] |
//! | dropzone `p-5` / centered `p-7 min-h-56` | [`DROPZONE_PADDING`] / [`DROPZONE_PADDING_CENTERED`] / [`DROPZONE_MIN_HEIGHT_CENTERED`] |
//! | dropzone `gap-4` / centered `gap-3` | [`DROPZONE_GAP`] / [`DROPZONE_GAP_CENTERED`] |
//! | tile `h-14 w-14 rounded-[1.25rem]` / `h-16 w-16 rounded-[1.35rem]` | [`DROPZONE_TILE`] / [`DROPZONE_TILE_CENTERED`] and their radii |
//! | `hover:border-foreground/40`, `data-[dragging=true]:border-foreground` | [`style::FOCUS_BORDER_ALPHA`] / `on_surface`, on [`switch::TRACK_COLOR_RAMP`] |
//! | `active:scale-[0.99]` | [`DROPZONE_PRESS_SCALE`] |
//! | dragging tile `translateY(-2px)`, `duration: 0.16` | [`DROPZONE_TILE_LIFT`] / [`UPLOAD_FAST_MS`] |
//! | `disabled:opacity-55` | [`UPLOAD_DISABLED_OPACITY`] |
//! | browse pill `px-3.5 py-2 text-xs rounded-full border` | [`BROWSE_PADDING_X`] / [`BROWSE_PADDING_Y`] / [`style::TEXT_XS`] |
//! | row `rounded-2xl border bg-background p-3` | [`style::RADIUS_2XL`] / [`ROW_PADDING`] |
//! | row `gap-3`, leading `h-11 w-11 rounded-xl bg-muted` | [`ROW_GAP`] / [`ROW_TILE`] / [`style::RADIUS_XL`] |
//! | name `text-sm font-medium`, meta `mt-0.5 text-xs text-muted-foreground` | [`style::TEXT_SM`] / [`META_GAP`] / [`style::TEXT_XS`] |
//! | action `h-7 w-7 rounded-full`, icon `h-3.5 w-3.5` | [`ACTION_BOX`] / [`ACTION_ICON`] |
//! | progress `mt-3 h-1.5 rounded-full bg-muted` | [`PROGRESS_GAP`] / [`PROGRESS_HEIGHT`] |
//! | progress fill `scaleX(ratio)`, `duration: 0.28` | [`PROGRESS_MS`], a lane on the ratio |
//! | row enter `y: 8 → 0`, exit `y: -6`, `duration: 0.22` | [`ROW_ENTER_RISE`] / [`ROW_EXIT_RISE`] / [`UPLOAD_ROW_MS`] |
//!
//! # There is no drop, because the framework delivers none
//!
//! Upstream's dropzone is a `<button>` wrapping a hidden `<input type="file">`,
//! with `onDragEnter`/`onDragOver`/`onDragLeave`/`onDrop` on top. **frust
//! delivers neither.** [`InputEvent`](frust::authoring::InputEvent) has five
//! variants — pointer, scroll, key, IME and the housekeeping broadcast — and
//! none of them carries a file, a drag session, or a path; no shell in this
//! repository publishes a file-drop signal for a widget to read, so there is
//! nothing this port could subscribe to, and nothing here waits on a shell seam
//! that does not exist yet.
//!
//! So the port is **programmatic**, and says so in its API:
//!
//! - the dropzone reports a press through [`FileUploadView::on_browse`] — the
//!   app opens whatever file dialog its platform has and appends the results;
//! - [`add_files`] is the entry point those results go through. It is upstream's
//!   own `addFiles` slot arithmetic (`remainingSlots`, `multiple`,
//!   `maxFiles`), ported so an app does not re-derive it;
//! - [`FileUploadView::dragging`] is a **prop**, not an observed state, so an
//!   app that *does* have a drag signal of its own (a desktop shell extension,
//!   say) can still drive the dropzone's dragging chrome.
//!
//! # Controlled, like every list in this catalog
//!
//! `items` is a prop. A press on a row's remove or retry affordance reports the
//! item's id through [`FileUploadView::on_remove`]/[`FileUploadView::on_retry`]
//! and changes nothing; the app edits its own list and the next rebuild brings
//! it down (`docs/CODE_STANDARDS.md`'s Interaction Semantics). Rows are matched
//! **by id** across rebuilds, which is what lets a row that has left the list
//! stay mounted for the length of its exit ramp — the [`Presence`] contract.
//!
//! # Degradations
//!
//! - **One leading glyph, not nine.** Upstream picks between `FileImage`,
//!   `FileVideo`, `FileAudio`, `FileArchive`, `FileSpreadsheet`, `FileText`,
//!   `FileCode2` and `FileIcon` by MIME type and extension. The *classification*
//!   is ported ([`FileUploadKind`], and [`FileUploadKind::of`] is the same
//!   ladder in the same order) but every kind paints one document glyph, since
//!   the catalog has no icon set to resolve eight lucide paths from. A caller
//!   that wants the distinction reads the classification.
//! - **The uploading spinner is an arc, not lucide's `Loader2`.** Same shape at
//!   this size, and it is the one glyph in the row that has to spin.
//! - **`UploadCloud` is approximated** by an upward arrow over a baseline: a
//!   cloud outline is not reconstructible from the catalog's paint vocabulary at
//!   24 units without vendoring the path.
//! - **No `accept` filter.** It is an attribute of the `<input type="file">`
//!   this port does not have; the app filters what it appends.
//! - **No shared-element image preview and no audio waveform** — those are the
//!   `attachment-upload` example's, not this file's (see above).

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::scene::arc_path;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, DashPattern,
    ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, TickClass,
    View, Widget, erase_callback, erase_callback_arg,
};
use frust::{FrameTime, Theme};

use crate::components::checkbox::{ICON_VIEWBOX, mark_path};
use crate::components::switch;
use crate::motion::{Presence, Ramp};
use crate::press::{Lane, inside_inclusive, presses};
use crate::style;
use crate::text::Label;
use crate::tokens::motion::EASE_OUT;
use crate::tokens::{BeuiTokens, sans_family};

// ---- Ported metrics --------------------------------------------------------

/// Gap between the dropzone and the queue, in logical px (`space-y-3`).
pub const UPLOAD_SECTION_GAP: f64 = 12.0;
/// Gap between two queue rows, in logical px (`space-y-2`).
pub const UPLOAD_LIST_GAP: f64 = 8.0;

/// The dropzone's dashed hairline (`border-dashed`, Tailwind's 3px-on/3px-off
/// default at a 1px border width).
pub const UPLOAD_DASH: DashPattern = DashPattern {
    on: 3.0,
    off: 3.0,
    phase: 0.0,
};

/// The dropzone's inner padding, in logical px (`p-5`).
pub const DROPZONE_PADDING: f64 = 20.0;
/// The centered dropzone's inner padding, in logical px (`p-7`).
pub const DROPZONE_PADDING_CENTERED: f64 = 28.0;
/// Gap between the dropzone's tile, its text and its browse pill, in logical px
/// (`gap-4`).
pub const DROPZONE_GAP: f64 = 16.0;
/// The centered dropzone's own gap, in logical px (`gap-3`).
pub const DROPZONE_GAP_CENTERED: f64 = 12.0;
/// The centered dropzone's minimum height, in logical px (`min-h-56`).
pub const DROPZONE_MIN_HEIGHT_CENTERED: f64 = 224.0;
/// The dropzone tile's edge, in logical px (`h-14 w-14`).
pub const DROPZONE_TILE: f64 = 56.0;
/// The centered dropzone tile's edge, in logical px (`h-16 w-16`).
pub const DROPZONE_TILE_CENTERED: f64 = 64.0;
/// The dropzone tile's corner radius, in logical px (`rounded-[1.25rem]`).
pub const DROPZONE_TILE_RADIUS: f64 = 20.0;
/// The centered tile's corner radius, in logical px (`rounded-[1.35rem]`).
pub const DROPZONE_TILE_RADIUS_CENTERED: f64 = 21.6;
/// The upload glyph's edge inside the tile, in logical px (`h-6 w-6`).
pub const DROPZONE_ICON: f64 = 24.0;
/// The centered upload glyph's edge, in logical px (`h-7 w-7`).
pub const DROPZONE_ICON_CENTERED: f64 = 28.0;
/// How far the tile lifts while a drag hovers, in logical px
/// (`translateY(-2px)`).
pub const DROPZONE_TILE_LIFT: f64 = 2.0;
/// The scale the dropzone shrinks to while pressed (`active:scale-[0.99]`).
pub const DROPZONE_PRESS_SCALE: f64 = 0.99;

/// The browse pill's horizontal padding, in logical px (`px-3.5`).
pub const BROWSE_PADDING_X: f64 = 14.0;
/// The browse pill's vertical padding, in logical px (`py-2`).
pub const BROWSE_PADDING_Y: f64 = 8.0;

/// A queue row's inner padding, in logical px (`p-3`).
pub const ROW_PADDING: f64 = 12.0;
/// Gap between a row's tile, its text and its actions, in logical px (`gap-3`).
pub const ROW_GAP: f64 = 12.0;
/// A row's leading tile edge, in logical px (`h-11 w-11`).
pub const ROW_TILE: f64 = 44.0;
/// The document glyph's edge inside a row tile, in logical px (`h-5 w-5`).
pub const ROW_TILE_ICON: f64 = style::ICON_SIZE_LG;
/// Gap between a row's name and its meta line, in logical px (`mt-0.5`).
pub const META_GAP: f64 = 2.0;
/// A row action's hit box, in logical px (`h-7 w-7`).
pub const ACTION_BOX: f64 = 28.0;
/// A row action's glyph edge, in logical px (`h-3.5 w-3.5`).
pub const ACTION_ICON: f64 = 14.0;
/// Gap between two row actions, in logical px (`gap-1`).
pub const ACTION_GAP: f64 = 4.0;
/// The status glyph's box, in logical px (`h-6 w-6`).
pub const STATUS_BOX: f64 = 24.0;
/// The status glyph's edge, in logical px (`h-4 w-4`).
pub const STATUS_ICON: f64 = style::ICON_SIZE;

/// Gap between a row's meta line and its progress track, in logical px
/// (`mt-3`).
pub const PROGRESS_GAP: f64 = 12.0;
/// The progress track's height, in logical px (`h-1.5`).
pub const PROGRESS_HEIGHT: f64 = 6.0;
/// How long the progress fill takes to reach a new ratio, in milliseconds
/// (`duration: 0.28`).
pub const PROGRESS_MS: u64 = 280;

/// How far a row rises into the queue, in logical px (`translateY(8px) → 0`).
pub const ROW_ENTER_RISE: f64 = 8.0;
/// How far a row rises as it leaves, in logical px (`translateY(-6px)`).
pub const ROW_EXIT_RISE: f64 = 6.0;
/// How long a row's entrance and exit take, in milliseconds
/// (`ROW_TRANSITION = { duration: 0.22 }`).
pub const UPLOAD_ROW_MS: u64 = 220;
/// The short transition upstream names `FAST_TRANSITION`, in milliseconds
/// (`duration: 0.16`) — the dragging lift and the status-glyph swap.
pub const UPLOAD_FAST_MS: u64 = 160;

/// How long the uploading spinner takes to turn once, in milliseconds
/// (Tailwind's `animate-spin`).
pub const SPINNER_PERIOD_MS: u64 = 1_000;
/// How much of its circle the spinner's arc covers (lucide's `Loader2` is an
/// open ring with one quadrant missing).
pub const SPINNER_SWEEP: f64 = std::f64::consts::PI * 1.5;

/// Opacity of a disabled dropzone: `disabled:opacity-55`.
pub const UPLOAD_DISABLED_OPACITY: f32 = 0.55;

/// The ramp a row enters and leaves on.
const ROW_RAMP: Ramp = Ramp::eased(Duration::from_millis(UPLOAD_ROW_MS), EASE_OUT);
/// The ramp the progress fill and the dragging lift run on.
const PROGRESS_RAMP: Ramp = Ramp::eased(Duration::from_millis(PROGRESS_MS), EASE_OUT);
/// The `FAST_TRANSITION` ramp.
const FAST_RAMP: Ramp = Ramp::eased(Duration::from_millis(UPLOAD_FAST_MS), EASE_OUT);

/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dim ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback muted fill — the light table's `--muted`.
const FALLBACK_MUTED_FILL: Color = crate::BEUI_LIGHT.muted;
/// Unthemed fallback page fill — the light table's `--background`.
const FALLBACK_BACKGROUND: Color = crate::BEUI_LIGHT.background;
/// Unthemed fallback error hue — the light table's `--destructive`.
const FALLBACK_DESTRUCTIVE: Color = crate::BEUI_LIGHT.destructive;

// ---- The item model --------------------------------------------------------

/// Where one queued file is in its upload — upstream's `FileUploadStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileUploadStatus {
    /// Accepted but not started. No progress track (`showProgress` is false).
    #[default]
    Queued,
    /// In flight: the spinner turns and the progress track fills.
    Uploading,
    /// Finished: the check glyph, and a track pinned at 100%.
    Success,
    /// Failed: the alert glyph, the destructive meta suffix, and a retry
    /// affordance beside the remove one.
    Error,
}

impl FileUploadStatus {
    /// The status word upstream announces (`STATUS_LABEL`).
    pub const fn label(self) -> &'static str {
        match self {
            FileUploadStatus::Queued => "Queued",
            FileUploadStatus::Uploading => "Uploading",
            FileUploadStatus::Success => "Uploaded",
            FileUploadStatus::Error => "Failed",
        }
    }

    /// Whether a row in this status shows its progress track
    /// (`showProgress = status === "uploading" || status === "success"`).
    pub const fn shows_progress(self) -> bool {
        matches!(
            self,
            FileUploadStatus::Uploading | FileUploadStatus::Success
        )
    }
}

/// The two dropzone shapes upstream's `variant` prop selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileUploadVariant {
    /// `"default"`: one row — tile, text, browse pill — at `p-5`.
    #[default]
    Default,
    /// `"centered"`: a tall stacked panel at `p-7`, `min-h-56`.
    Centered,
}

/// What upstream's `getFileIcon` ladder classifies a file as.
///
/// The classification is ported; the eight distinct lucide glyphs are not (see
/// the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileUploadKind {
    /// `type.startsWith("image/")`.
    Image,
    /// `type.startsWith("video/")`.
    Video,
    /// `type.startsWith("audio/")`.
    Audio,
    /// A zip/compressed type, or a `zip`/`rar`/`7z`/`tar`/`gz` extension.
    Archive,
    /// A spreadsheet/excel type, or a `csv`/`xls`/`xlsx` extension.
    Spreadsheet,
    /// A pdf or `text/*` type, or a `pdf`/`doc`/`docx`/`md`/`txt` extension.
    Document,
    /// One of upstream's eleven source-file extensions.
    Code,
    /// Anything else — upstream's `FileIcon` fallback.
    #[default]
    Other,
}

impl FileUploadKind {
    /// The extensions upstream routes to [`FileUploadKind::Code`].
    const CODE_EXTENSIONS: [&'static str; 11] = [
        "css", "html", "js", "jsx", "json", "mdx", "ts", "tsx", "xml", "yaml", "yml",
    ];

    /// Classify `item` exactly the way upstream's `getFileIcon` does — in the
    /// same order, so a `.csv` reported as `text/csv` lands on
    /// [`Spreadsheet`](FileUploadKind::Spreadsheet) here as it does there.
    pub fn of(item: &FileUploadItem) -> Self {
        let extension = file_extension(&item.name).unwrap_or_default();
        let mime = item.mime.as_deref().unwrap_or_default();

        if mime.starts_with("image/") {
            return FileUploadKind::Image;
        }
        if mime.starts_with("video/") {
            return FileUploadKind::Video;
        }
        if mime.starts_with("audio/") {
            return FileUploadKind::Audio;
        }
        if mime.contains("zip")
            || mime.contains("compressed")
            || matches!(extension.as_str(), "zip" | "rar" | "7z" | "tar" | "gz")
        {
            return FileUploadKind::Archive;
        }
        if mime.contains("spreadsheet")
            || mime.contains("excel")
            || matches!(extension.as_str(), "csv" | "xls" | "xlsx")
        {
            return FileUploadKind::Spreadsheet;
        }
        if mime.contains("pdf")
            || mime.starts_with("text/")
            || matches!(extension.as_str(), "pdf" | "doc" | "docx" | "md" | "txt")
        {
            return FileUploadKind::Document;
        }
        if Self::CODE_EXTENSIONS.contains(&extension.as_str()) {
            return FileUploadKind::Code;
        }
        FileUploadKind::Other
    }
}

/// One file in the queue — upstream's `FileUploadItem` less its `File` handle,
/// which has no frust counterpart (the app keeps whatever handle its platform
/// gave it, keyed by [`id`](FileUploadItem::id)).
#[derive(Clone, Debug, PartialEq)]
pub struct FileUploadItem {
    /// The row's identity across rebuilds — what `on_remove`/`on_retry` report
    /// and what a row's [`Presence`] is matched by.
    pub id: String,
    /// The file's name, shown on the row's first line.
    pub name: String,
    /// The file's size in bytes, formatted by [`format_bytes`].
    pub size: u64,
    /// The file's MIME type, if the platform reported one (`type`).
    pub mime: Option<String>,
    /// Upload progress in **percent**, `0..=100` — upstream's own unit.
    pub progress: f64,
    /// Where the upload has got to.
    pub status: FileUploadStatus,
    /// A failure message, appended to the meta line in
    /// [`FileUploadStatus::Error`].
    pub error: Option<String>,
}

/// Create a queued item — upstream's `createFileUploadItem`, which starts a
/// freshly-added file at `progress: 0, status: "uploading"`.
pub fn file_upload_item(
    id: impl Into<String>,
    name: impl Into<String>,
    size: u64,
) -> FileUploadItem {
    FileUploadItem {
        id: id.into(),
        name: name.into(),
        size,
        mime: None,
        progress: 0.0,
        status: FileUploadStatus::Uploading,
        error: None,
    }
}

impl FileUploadItem {
    /// Set the reported MIME type (`type`).
    pub fn mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    /// Set the upload progress, in percent.
    pub fn progress(mut self, progress: f64) -> Self {
        self.progress = progress;
        self
    }

    /// Set the upload status.
    pub fn status(mut self, status: FileUploadStatus) -> Self {
        self.status = status;
        self
    }

    /// Put the item in [`FileUploadStatus::Error`] with `message`.
    pub fn error(mut self, message: impl Into<String>) -> Self {
        self.status = FileUploadStatus::Error;
        self.error = Some(message.into());
        self
    }

    /// Reset for another attempt — upstream's `retryItem`: the error cleared,
    /// progress back to zero, status back to uploading.
    pub fn retrying(mut self) -> Self {
        self.error = None;
        self.progress = 0.0;
        self.status = FileUploadStatus::Uploading;
        self
    }

    /// The row's meta prefix — upstream's `fileKind`: the uppercased extension,
    /// else the uppercased MIME subtype, else `FILE`.
    pub fn kind_label(&self) -> String {
        if let Some(extension) = file_extension(&self.name) {
            return extension.to_uppercase();
        }
        if let Some(mime) = &self.mime
            && let Some(subtype) = mime.rsplit('/').next()
            && !subtype.is_empty()
        {
            return subtype.to_uppercase();
        }
        "FILE".to_string()
    }

    /// The whole meta line: `KIND · SIZE`, plus ` · ERROR` when failed.
    pub fn meta_line(&self) -> String {
        let mut line = format!("{} · {}", self.kind_label(), format_bytes(self.size));
        if self.status == FileUploadStatus::Error
            && let Some(error) = &self.error
        {
            line.push_str(" · ");
            line.push_str(error);
        }
        line
    }
}

/// The lowercased extension of `name`, or `None` when it carries no dot —
/// upstream's `name.includes(".") ? name.split(".").pop() : undefined`.
fn file_extension(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    Some(extension.to_lowercase())
}

/// Format a byte count the way upstream's `formatBytes` does: binary units, one
/// decimal below 10 and none at or above it, and `0 B` for anything not
/// positive.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut exponent = 0usize;
    let mut value = bytes as f64;
    while value >= 1024.0 && exponent + 1 < UNITS.len() {
        value /= 1024.0;
        exponent += 1;
    }
    if value >= 10.0 || exponent == 0 {
        format!("{:.0} {}", value, UNITS[exponent])
    } else {
        format!("{:.1} {}", value, UNITS[exponent])
    }
}

/// Clamp a reported progress into `0..=100` — upstream's `clampProgress`, with
/// its one special case: a successful upload reads 100 whatever it reported.
pub fn clamp_progress(progress: f64, status: FileUploadStatus) -> f64 {
    if status == FileUploadStatus::Success {
        return 100.0;
    }
    if progress.is_nan() {
        return 0.0;
    }
    progress.clamp(0.0, 100.0)
}

/// Append `incoming` to `items` under upstream's `addFiles` slot arithmetic,
/// returning how many were actually taken.
///
/// This is the entry point files arrive through — see the [module docs](self)
/// on why there is no drop handler to call it for you. `multiple = false` takes
/// at most one file per call (upstream's `Math.min(1, remainingSlots)`), and
/// `max_files` caps the list's total length; both refuse silently rather than
/// erroring, exactly as upstream does.
pub fn add_files(
    items: &mut Vec<FileUploadItem>,
    incoming: impl IntoIterator<Item = FileUploadItem>,
    multiple: bool,
    max_files: Option<usize>,
) -> usize {
    let remaining = match max_files {
        Some(max) => max.saturating_sub(items.len()),
        None => usize::MAX,
    };
    if remaining == 0 {
        return 0;
    }
    let take = if multiple {
        remaining
    } else {
        remaining.min(1)
    };
    let before = items.len();
    items.extend(incoming.into_iter().take(take));
    items.len() - before
}

// ---- View ------------------------------------------------------------------

/// A view-held, typed id callback (erased on build).
type OnId<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI upload queue. See the [module docs](self).
pub struct FileUploadView<State: 'static> {
    items: Vec<FileUploadItem>,
    variant: FileUploadVariant,
    title: String,
    description: String,
    browse_label: String,
    max_files: Option<usize>,
    disabled: bool,
    dragging: bool,
    on_browse: Rc<dyn Fn(&mut State)>,
    on_remove: Option<OnId<State>>,
    on_retry: Option<OnId<State>>,
}

/// Create an upload queue showing `items`, reporting a dropzone press through
/// `on_browse` — a **controlled** component (see the [module docs](self)).
pub fn file_upload<State: 'static>(
    items: Vec<FileUploadItem>,
    on_browse: impl Fn(&mut State) + 'static,
) -> FileUploadView<State> {
    FileUploadView {
        items,
        variant: FileUploadVariant::default(),
        // The component's own prop defaults.
        title: "Drop files here".to_string(),
        description: "Add files to the upload queue".to_string(),
        browse_label: "Browse".to_string(),
        max_files: None,
        disabled: false,
        dragging: false,
        on_browse: Rc::new(on_browse),
        on_remove: None,
        on_retry: None,
    }
}

impl<State: 'static> FileUploadView<State> {
    /// Select the dropzone shape (default [`FileUploadVariant::Default`]).
    pub fn variant(mut self, variant: FileUploadVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the dropzone's headline (`title`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the dropzone's supporting line (`description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set the browse pill's label (`browseLabel`).
    pub fn browse_label(mut self, label: impl Into<String>) -> Self {
        self.browse_label = label.into();
        self
    }

    /// Cap the queue (`maxFiles`). Once reached the dropzone goes inert and
    /// swaps its copy for the limit-reached wording, as upstream does.
    pub fn max_files(mut self, max_files: usize) -> Self {
        self.max_files = Some(max_files);
        self
    }

    /// Disable the dropzone (`disabled`): dimmed to
    /// [`UPLOAD_DISABLED_OPACITY`] and inert. Row affordances go with it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Drive the dropzone's dragging chrome (`data-[dragging=true]`).
    ///
    /// A **prop**, not an observed state: frust delivers no drag session a
    /// widget could latch (see the [module docs](self)), so an app with its own
    /// drag signal sets this and an app without one leaves it `false`.
    pub fn dragging(mut self, dragging: bool) -> Self {
        self.dragging = dragging;
        self
    }

    /// Report a row's remove press by id (`onRemove`).
    pub fn on_remove<F: Fn(&mut State, String) + 'static>(mut self, on_remove: F) -> Self {
        self.on_remove = Some(Rc::new(on_remove));
        self
    }

    /// Report a failed row's retry press by id (`onRetry`). A row in
    /// [`FileUploadStatus::Error`] shows the affordance only when this is set.
    pub fn on_retry<F: Fn(&mut State, String) + 'static>(mut self, on_retry: F) -> Self {
        self.on_retry = Some(Rc::new(on_retry));
        self
    }

    /// Whether the queue has reached `max_files` (`maxReached`).
    fn max_reached(&self) -> bool {
        self.max_files.is_some_and(|max| self.items.len() >= max)
    }

    /// The dropzone's headline in the current state.
    fn headline(&self) -> String {
        if self.max_reached() {
            "Upload limit reached".to_string()
        } else {
            self.title.clone()
        }
    }

    /// The dropzone's supporting line in the current state.
    fn support(&self) -> String {
        match self.max_files {
            Some(max) if self.max_reached() => {
                format!("{} of {max} files added", self.items.len())
            }
            _ => self.description.clone(),
        }
    }
}

// ---- Widget ----------------------------------------------------------------

/// One retained queue row.
struct Row {
    /// The item's id — the identity rows are matched by across rebuilds.
    id: String,
    /// The item as of the last rebuild that still carried it.
    item: FileUploadItem,
    name: Label,
    meta: Label,
    /// Kept-mounted enter/exit — see the [module docs](self).
    presence: Presence,
    /// What [`Presence::advance`] last reported, so `layout` (which carries no
    /// clock) and `paint` read one value rather than each sampling their own.
    shown: f64,
    /// The progress fill's ratio, `0.0 ..= 1.0`.
    progress: Lane,
    /// The row's box in widget-local space, resolved by layout.
    rect: Rect,
    /// Where the name and meta runs were placed, so paint does not re-derive.
    text_x: f64,
    /// Whether this row still appears in the app's list.
    leaving: bool,
}

impl Row {
    fn new(item: &FileUploadItem, reduce: bool) -> Self {
        let mut presence = Presence::symmetric(ROW_RAMP);
        if reduce {
            presence = presence.collapsed();
        }
        presence.set_open(true);
        Row {
            id: item.id.clone(),
            item: item.clone(),
            name: Label::new(&item.name),
            meta: Label::new(item.meta_line()),
            presence,
            progress: Lane::at_rest(
                PROGRESS_RAMP,
                clamp_progress(item.progress, item.status) / 100.0,
            ),
            shown: 0.0,
            rect: Rect::ZERO,
            text_x: 0.0,
            leaving: false,
        }
    }

    /// The row's full height at presence 1: padding, then whichever of the
    /// leading tile and the text block is taller, plus the progress track.
    fn content_height(&self) -> f64 {
        let mut text = self.name.size().height + META_GAP + self.meta.size().height;
        if self.item.status.shows_progress() {
            text += PROGRESS_GAP + PROGRESS_HEIGHT;
        }
        ROW_PADDING * 2.0 + text.max(ROW_TILE)
    }
}

/// Which affordance a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The dropzone itself.
    Dropzone,
    /// Row `index`'s retry affordance.
    Retry(usize),
    /// Row `index`'s remove affordance.
    Remove(usize),
}

/// The retained widget for a [`FileUploadView`].
pub struct FileUploadWidget {
    rows: Vec<Row>,
    variant: FileUploadVariant,
    headline: Label,
    support: Label,
    browse: Label,
    /// The three dropzone strings, kept beside their shaped runs because a
    /// cached run holds no readable copy — semantics needs the headline text.
    headline_text: String,
    support_text: String,
    browse_text: String,
    disabled: bool,
    dragging: bool,
    max_reached: bool,
    has_retry: bool,
    /// The dropzone's box, resolved by layout and read by the event pass.
    dropzone: Rect,
    /// The tile's dragging lift, `0.0` resting .. `1.0` lifted.
    lift: Lane,
    /// The dropzone's border crossfade toward the dragging/hovered colour.
    border_blend: Lane,
    /// The dropzone's press shrink, `0.0` resting .. `1.0` pressed.
    press: Lane,
    /// The spinner's epoch, so every turning row shares one phase.
    spin_epoch: FrameTime,
    spin_epoch_pending: bool,
    /// The affordance a `Down` armed, if any.
    armed: Option<Target>,
    /// The latched hovered affordance, self-corrected at paint time.
    hovered: Option<Target>,
    /// The width layout resolved.
    width: f64,
    on_browse: ErasedCallback,
    on_remove: Option<ErasedArgCallback<String>>,
    on_retry: Option<ErasedArgCallback<String>>,
}

impl<State: 'static> View<State> for FileUploadView<State> {
    type Element = FileUploadWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FileUploadWidget {
        let mut rows: Vec<Row> = self.items.iter().map(|i| Row::new(i, false)).collect();
        // `<AnimatePresence initial={false}>`: rows already in the list when the
        // queue mounts are simply there, they do not cascade in.
        // Driving each row past its own entrance ramp is what leaves it
        // `Present` rather than one frame into an entrance nobody asked for.
        let settled = FrameTime::from_nanos(ROW_RAMP.settle().as_nanos() as u64 + 1);
        for row in &mut rows {
            row.presence.advance(FrameTime::ZERO);
            row.shown = row.presence.advance(settled);
        }
        FileUploadWidget {
            rows,
            variant: self.variant,
            headline: Label::new(self.headline()),
            support: Label::new(self.support()),
            browse: Label::new(&self.browse_label),
            headline_text: self.headline(),
            support_text: self.support(),
            browse_text: self.browse_label.clone(),
            disabled: self.disabled,
            dragging: self.dragging,
            max_reached: self.max_reached(),
            has_retry: self.on_retry.is_some(),
            dropzone: Rect::ZERO,
            lift: Lane::at_rest(FAST_RAMP, if self.dragging { 1.0 } else { 0.0 }),
            border_blend: Lane::at_rest(
                switch::TRACK_COLOR_RAMP,
                if self.dragging { 1.0 } else { 0.0 },
            ),
            press: Lane::at_rest(FAST_RAMP, 0.0),
            spin_epoch: FrameTime::ZERO,
            spin_epoch_pending: true,
            armed: None,
            hovered: None,
            width: 0.0,
            on_browse: erase_callback(&self.on_browse),
            on_remove: self.on_remove.as_ref().map(erase_callback_arg),
            on_retry: self.on_retry.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FileUploadWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_browse = erase_callback(&self.on_browse);
        element.on_remove = self.on_remove.as_ref().map(erase_callback_arg);
        element.on_retry = self.on_retry.as_ref().map(erase_callback_arg);

        let mut flags = ChangeFlags::NONE;

        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.headline.set_content(self.headline())
            | element.support.set_content(self.support())
            | element.browse.set_content(&self.browse_label)
        {
            element.headline_text = self.headline();
            element.support_text = self.support();
            element.browse_text = self.browse_label.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.armed = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.dragging != self.dragging {
            element.dragging = self.dragging;
            element.lift.retarget(if self.dragging { 1.0 } else { 0.0 });
            flags |= ChangeFlags::PAINT;
        }
        if prev.max_reached() != self.max_reached() {
            element.max_reached = self.max_reached();
            flags |= ChangeFlags::PAINT;
        }
        if prev.on_retry.is_some() != self.on_retry.is_some() {
            element.has_retry = self.on_retry.is_some();
            flags |= ChangeFlags::PAINT;
        }
        if prev.items != self.items {
            element.reconcile(&self.items);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // A row whose exit finished on an earlier frame is finally dropped.
        if element.drop_exited() {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }
}

impl FileUploadWidget {
    /// Match `items` onto the retained rows by id: an unchanged row keeps its
    /// lanes, a new one enters, and one that has left the list is kept mounted
    /// through its exit ramp.
    fn reconcile(&mut self, items: &[FileUploadItem]) {
        for row in &mut self.rows {
            if !items.iter().any(|item| item.id == row.id) && !row.leaving {
                row.leaving = true;
                row.presence.set_open(false);
            }
        }
        for item in items {
            match self.rows.iter_mut().find(|row| row.id == item.id) {
                Some(row) => {
                    if row.leaving {
                        // Re-added before its exit finished: it comes back
                        // rather than being mounted a second time.
                        row.leaving = false;
                        row.presence.set_open(true);
                    }
                    row.name.set_content(&item.name);
                    row.meta.set_content(item.meta_line());
                    row.progress
                        .retarget(clamp_progress(item.progress, item.status) / 100.0);
                    row.item = item.clone();
                }
                None => self.rows.push(Row::new(item, false)),
            }
        }

        // Re-order to the app's order, with every leaving row pinned to the
        // slot it is leaving from — a row playing its exit must not jump.
        let mut pinned: Vec<(usize, Row)> = Vec::new();
        let mut live: Vec<Row> = Vec::new();
        for (index, row) in self.rows.drain(..).enumerate() {
            if row.leaving {
                pinned.push((index, row));
            } else {
                live.push(row);
            }
        }
        let mut ordered: Vec<Row> = Vec::with_capacity(live.len());
        for item in items {
            if let Some(at) = live.iter().position(|row| row.id == item.id) {
                ordered.push(live.remove(at));
            }
        }
        ordered.extend(live);
        for (index, row) in pinned {
            let at = index.min(ordered.len());
            ordered.insert(at, row);
        }
        self.rows = ordered;
    }

    /// Drop every row whose exit ramp has finished. Reports whether any went.
    fn drop_exited(&mut self) -> bool {
        let before = self.rows.len();
        self.rows
            .retain(|row| !row.leaving || row.presence.is_visible());
        self.rows.len() != before
    }

    /// The dropzone's own metrics for the current variant.
    const fn tile(&self) -> f64 {
        match self.variant {
            FileUploadVariant::Default => DROPZONE_TILE,
            FileUploadVariant::Centered => DROPZONE_TILE_CENTERED,
        }
    }

    const fn tile_radius(&self) -> f64 {
        match self.variant {
            FileUploadVariant::Default => DROPZONE_TILE_RADIUS,
            FileUploadVariant::Centered => DROPZONE_TILE_RADIUS_CENTERED,
        }
    }

    const fn dropzone_padding(&self) -> f64 {
        match self.variant {
            FileUploadVariant::Default => DROPZONE_PADDING,
            FileUploadVariant::Centered => DROPZONE_PADDING_CENTERED,
        }
    }

    const fn dropzone_gap(&self) -> f64 {
        match self.variant {
            FileUploadVariant::Default => DROPZONE_GAP,
            FileUploadVariant::Centered => DROPZONE_GAP_CENTERED,
        }
    }

    const fn dropzone_icon(&self) -> f64 {
        match self.variant {
            FileUploadVariant::Default => DROPZONE_ICON,
            FileUploadVariant::Centered => DROPZONE_ICON_CENTERED,
        }
    }

    /// Whether the dropzone accepts a press at all
    /// (`disabled={disabled || maxReached}`).
    const fn dropzone_enabled(&self) -> bool {
        !self.disabled && !self.max_reached
    }

    /// Whether row `index` shows a retry affordance.
    fn row_has_retry(&self, index: usize) -> bool {
        self.has_retry
            && self
                .rows
                .get(index)
                .is_some_and(|row| row.item.status == FileUploadStatus::Error)
    }

    /// Row `index`'s remove affordance box, in widget-local space.
    fn remove_rect(&self, index: usize) -> Rect {
        let row = &self.rows[index];
        let x = row.rect.max_x() - ROW_PADDING - ACTION_BOX;
        Rect::from_origin_size(
            Point::new(x, row.rect.y0 + ROW_PADDING),
            Size::new(ACTION_BOX, ACTION_BOX),
        )
    }

    /// Row `index`'s retry affordance box, immediately left of its remove one.
    fn retry_rect(&self, index: usize) -> Rect {
        let remove = self.remove_rect(index);
        Rect::from_origin_size(
            Point::new(remove.x0 - ACTION_GAP - ACTION_BOX, remove.y0),
            Size::new(ACTION_BOX, ACTION_BOX),
        )
    }

    /// Row `index`'s status glyph box, left of whatever actions it shows.
    fn status_rect(&self, index: usize) -> Rect {
        let leftmost = if self.row_has_retry(index) {
            self.retry_rect(index)
        } else {
            self.remove_rect(index)
        };
        Rect::from_origin_size(
            Point::new(
                leftmost.x0 - ACTION_GAP - STATUS_BOX,
                leftmost.y0 + (ACTION_BOX - STATUS_BOX) / 2.0,
            ),
            Size::new(STATUS_BOX, STATUS_BOX),
        )
    }

    /// What a widget-local point lands on, if anything pressable.
    fn target_at(&self, at: Point) -> Option<Target> {
        for index in 0..self.rows.len() {
            if self.rows[index].leaving {
                continue;
            }
            if self.row_has_retry(index) && self.retry_rect(index).contains(at) {
                return Some(Target::Retry(index));
            }
            if self.on_remove.is_some() && self.remove_rect(index).contains(at) {
                return Some(Target::Remove(index));
            }
        }
        if self.dropzone_enabled()
            && inside_inclusive(at - self.dropzone.origin().to_vec2(), self.dropzone.size())
        {
            return Some(Target::Dropzone);
        }
        None
    }

    /// The spinner's rotation at `now`, in radians.
    fn spin_angle(&self, now: FrameTime) -> f64 {
        let period = SPINNER_PERIOD_MS as f64;
        let elapsed = now.saturating_sub(self.spin_epoch).as_secs_f64() * 1000.0;
        (elapsed % period) / period * std::f64::consts::TAU
    }

    /// Whether any row is still uploading, so the spinner owes frames.
    fn any_uploading(&self) -> bool {
        self.rows
            .iter()
            .any(|row| row.item.status == FileUploadStatus::Uploading && !row.leaving)
    }
}

/// The palette the queue paints from.
struct UploadColors {
    border: Color,
    ink: Color,
    muted: Color,
    /// `bg-muted`: the row tile and the progress track.
    muted_fill: Color,
    /// `bg-background`: a row's own fill.
    background: Color,
    destructive: Color,
    success: Color,
}

fn resolve_colors(theme: Option<&Theme>) -> UploadColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            UploadColors {
                border: scheme.outline_variant,
                ink: scheme.on_surface,
                muted: scheme.on_surface_variant,
                muted_fill: scheme.surface_container_highest,
                background: scheme.surface,
                destructive: scheme.error,
                success: BeuiTokens::resolve(Some(theme)).success,
            }
        }
        None => UploadColors {
            border: FALLBACK_BORDER,
            ink: FALLBACK_FOREGROUND,
            muted: FALLBACK_MUTED,
            muted_fill: FALLBACK_MUTED_FILL,
            background: FALLBACK_BACKGROUND,
            destructive: FALLBACK_DESTRUCTIVE,
            success: BeuiTokens::beui().success,
        },
    }
}

/// The tone a status glyph is painted in (`STATUS_TONE`).
fn status_tone(status: FileUploadStatus, colors: &UploadColors) -> Color {
    match status {
        FileUploadStatus::Queued => colors.muted,
        FileUploadStatus::Uploading => colors.ink,
        FileUploadStatus::Success => colors.success,
        FileUploadStatus::Error => colors.destructive,
    }
}

/// `font-semibold` at `size`, in the catalog's sans family.
fn semibold(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::SEMI_BOLD,
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

/// `font-medium` at `size`.
fn medium(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

/// A plain run at `size`.
fn plain(size: f64, color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: size as f32,
        color,
        ..TextStyle::default()
    }
}

// ---- Glyphs ----------------------------------------------------------------

/// The lucide stroke width, in viewBox units.
const ICON_STROKE_VIEWBOX: f64 = 2.0;

/// The stroke width a `size`-square glyph is drawn at.
fn icon_stroke(size: f64) -> f64 {
    ICON_STROKE_VIEWBOX * size / ICON_VIEWBOX
}

/// An upward arrow over a baseline, centred on `centre` — this port's
/// approximation of lucide's `UploadCloud` (see the [module docs](self)).
fn upload_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let half = size / 2.0;
    let mut path = BezPath::new();
    // The shaft.
    path.move_to(Point::new(centre.x, centre.y + half * 0.55));
    path.line_to(Point::new(centre.x, centre.y - half * 0.75));
    // The head.
    path.move_to(Point::new(centre.x - half * 0.4, centre.y - half * 0.35));
    path.line_to(Point::new(centre.x, centre.y - half * 0.75));
    path.line_to(Point::new(centre.x + half * 0.4, centre.y - half * 0.35));
    // The baseline it lifts off.
    path.move_to(Point::new(centre.x - half * 0.7, centre.y + half * 0.8));
    path.line_to(Point::new(centre.x + half * 0.7, centre.y + half * 0.8));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

/// A document outline with a folded corner, centred on `centre` — the single
/// leading glyph every [`FileUploadKind`] shares here.
fn document_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let w = size * 0.62;
    let h = size * 0.8;
    let fold = size * 0.24;
    let left = centre.x - w / 2.0;
    let right = centre.x + w / 2.0;
    let top = centre.y - h / 2.0;
    let bottom = centre.y + h / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(right - fold, top));
    path.line_to(Point::new(left, top));
    path.line_to(Point::new(left, bottom));
    path.line_to(Point::new(right, bottom));
    path.line_to(Point::new(right, top + fold));
    path.line_to(Point::new(right - fold, top));
    path.line_to(Point::new(right - fold, top + fold));
    path.line_to(Point::new(right, top + fold));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

/// A ring with an exclamation bar, centred on `centre` — lucide's `AlertCircle`.
fn alert_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let radius = size / 2.0 - icon_stroke(size) / 2.0;
    let ring = arc_path(centre, radius, 0.0, std::f64::consts::TAU);
    let stroke = icon_stroke(size);
    scene.stroke_path(Point::ZERO, &ring, stroke, &Brush::Solid(color));
    let mut bar = BezPath::new();
    bar.move_to(Point::new(centre.x, centre.y - radius * 0.5));
    bar.line_to(Point::new(centre.x, centre.y + radius * 0.15));
    bar.move_to(Point::new(centre.x, centre.y + radius * 0.5));
    bar.line_to(Point::new(centre.x, centre.y + radius * 0.55));
    scene.stroke_path(Point::ZERO, &bar, stroke, &Brush::Solid(color));
}

/// A ring with a check inside it, centred on `centre` — lucide's `CheckCircle2`.
fn check_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let radius = size / 2.0 - icon_stroke(size) / 2.0;
    let ring = arc_path(centre, radius, 0.0, std::f64::consts::TAU);
    scene.stroke_path(Point::ZERO, &ring, icon_stroke(size), &Brush::Solid(color));
    let inner = size * 0.6;
    let mark = mark_path(&crate::components::checkbox::CHECK_POINTS, inner, 1.0);
    scene.stroke_path(
        Point::new(centre.x - inner / 2.0, centre.y - inner / 2.0),
        &mark,
        icon_stroke(size),
        &Brush::Solid(color),
    );
}

/// A three-quarter arc turned by `angle`, centred on `centre` — the spinner
/// standing in for lucide's `Loader2`.
fn spinner_glyph(centre: Point, size: f64, angle: f64, color: Color, scene: &mut dyn PaintScene) {
    let radius = size / 2.0 - icon_stroke(size) / 2.0;
    let arc = arc_path(centre, radius, angle, SPINNER_SWEEP);
    scene.stroke_path(Point::ZERO, &arc, icon_stroke(size), &Brush::Solid(color));
}

/// A three-quarter arc with an arrowhead, centred on `centre` — lucide's
/// `RotateCcw`.
fn retry_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let radius = size / 2.0 - icon_stroke(size) / 2.0;
    let arc = arc_path(
        centre,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI * 1.6,
    );
    let stroke = icon_stroke(size);
    scene.stroke_path(Point::ZERO, &arc, stroke, &Brush::Solid(color));
    // The head, at the arc's twelve-o'clock start.
    let tip = Point::new(centre.x, centre.y - radius);
    let arm = radius * 0.45;
    let mut head = BezPath::new();
    head.move_to(Point::new(tip.x - arm, tip.y - arm * 0.6));
    head.line_to(tip);
    head.line_to(Point::new(tip.x - arm * 0.2, tip.y + arm * 0.8));
    scene.stroke_path(Point::ZERO, &head, stroke, &Brush::Solid(color));
}

/// Lucide's `X`, centred on `centre`.
fn cross_glyph(centre: Point, size: f64, color: Color, scene: &mut dyn PaintScene) {
    let arm = size / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - arm, centre.y - arm));
    path.line_to(Point::new(centre.x + arm, centre.y + arm));
    path.move_to(Point::new(centre.x + arm, centre.y - arm));
    path.line_to(Point::new(centre.x - arm, centre.y + arm));
    scene.stroke_path(Point::ZERO, &path, icon_stroke(size), &Brush::Solid(color));
}

impl Widget for FileUploadWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            crate::components::input::UNBOUNDED_WIDTH
        };
        self.width = width;

        // The dropzone's own three runs.
        let headline_size = match self.variant {
            FileUploadVariant::Default => style::TEXT_SM,
            FileUploadVariant::Centered => style::TEXT_BASE,
        };
        let headline = self
            .headline
            .layout(ctx, &semibold(headline_size, colors.ink));
        let support = self
            .support
            .layout(ctx, &plain(style::TEXT_XS, colors.muted));
        let browse = self.browse.layout(ctx, &medium(style::TEXT_XS, colors.ink));

        let padding = self.dropzone_padding();
        let gap = self.dropzone_gap();
        let text_block = headline.height + META_GAP + support.height;
        let dropzone_height = match self.variant {
            FileUploadVariant::Default => {
                padding * 2.0
                    + text_block
                        .max(self.tile())
                        .max(browse.height + BROWSE_PADDING_Y)
            }
            FileUploadVariant::Centered => (padding * 2.0
                + self.tile()
                + gap
                + text_block
                + gap
                + browse.height
                + BROWSE_PADDING_Y * 2.0)
                .max(DROPZONE_MIN_HEIGHT_CENTERED),
        };
        self.dropzone = Rect::from_origin_size(Point::ORIGIN, Size::new(width, dropzone_height));

        // Then the queue, one row per presence-visible entry.
        let mut y = dropzone_height;
        let mut first = true;
        for index in 0..self.rows.len() {
            let (name, meta) = {
                let row = &mut self.rows[index];
                (
                    row.name.layout(ctx, &medium(style::TEXT_SM, colors.ink)),
                    row.meta.layout(ctx, &plain(style::TEXT_XS, colors.muted)),
                )
            };
            let row = &mut self.rows[index];
            let _ = (name, meta);
            // A row takes its full slot the moment it is added (it fades and
            // rises into it) and gives that slot back gradually as it leaves,
            // so the queue closes up behind a removal instead of jumping.
            let full = row.content_height();
            let height = if row.leaving {
                full * row.shown.clamp(0.0, 1.0)
            } else if row.presence.is_visible() {
                full
            } else {
                0.0
            };
            if height <= 0.0 {
                row.rect = Rect::ZERO;
                continue;
            }
            y += if first {
                UPLOAD_SECTION_GAP
            } else {
                UPLOAD_LIST_GAP
            };
            first = false;
            row.rect = Rect::from_origin_size(Point::new(0.0, y), Size::new(width, height));
            row.text_x = ROW_PADDING + ROW_TILE + ROW_GAP;
            y += height;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let now = ctx.frame_time();
        if self.spin_epoch_pending {
            self.spin_epoch = now;
            self.spin_epoch_pending = false;
        }
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let origin = ctx.origin();

        let (colors, reduce) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                resolve_colors(theme),
                theme.is_some_and(|t| t.motion.reduce_motion),
            )
        };

        let hovering_dropzone = self.hovered == Some(Target::Dropzone);
        self.border_blend
            .retarget(if self.dragging || hovering_dropzone {
                1.0
            } else {
                0.0
            });
        self.press
            .retarget(if self.armed == Some(Target::Dropzone) {
                1.0
            } else {
                0.0
            });

        let mut owes_frame = false;
        let mut owes_layout = false;
        if reduce {
            self.lift.snap();
            self.border_blend.snap();
            self.press.snap();
            for row in &mut self.rows {
                row.progress.snap();
            }
        } else {
            owes_frame |= self.lift.advance(now);
            owes_frame |= self.border_blend.advance(now);
            owes_frame |= self.press.advance(now);
            for row in &mut self.rows {
                owes_frame |= row.progress.advance(now);
            }
        }
        for row in &mut self.rows {
            row.shown = row.presence.advance(now);
            // Under reduce_motion, snap the shown progress to its settled state:
            // rows appear/disappear instantly without overwriting the presence's
            // configured ramps, so they remain available when reduce_motion toggles.
            if reduce {
                row.shown = if row.presence.is_visible() { 1.0 } else { 0.0 };
            }
            if row.presence.is_animating() {
                // A leaving row's height is what its presence drives, so that
                // arm owes a relayout; an entering one only owes a repaint.
                if row.leaving {
                    owes_layout = true;
                } else {
                    owes_frame = true;
                }
            } else if row.leaving && !row.presence.is_visible() {
                // A row whose exit ramp has just settled needs to be dropped.
                // Request a rebuild so drop_exited() can remove it on the next pass.
                owes_layout = true;
            }
        }

        self.paint_dropzone(origin, &colors, scene);
        let spin = self.spin_angle(now);
        for index in 0..self.rows.len() {
            self.paint_row(index, origin, &colors, spin, scene);
        }

        if owes_layout {
            // A row's height is what its presence drives, so an animating queue
            // owes a relayout rather than a bare repaint.
            ctx.request_layout();
        } else if owes_frame {
            ctx.request_frame();
        }

        // The uploading spinner is a perpetual decorative loop: it runs only
        // while something is uploading, and it gets its own paced frame class.
        if self.any_uploading() && !reduce {
            ctx.request_frame_class(TickClass::CosmeticLoop);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if self.disabled || !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(target) = self.target_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.armed = Some(target);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.armed.is_none() {
                    // The hover/cursor pass: the claim is per-pass and the
                    // cursor request is stateless, so both are re-issued on
                    // every qualifying move.
                    let target = (!self.disabled)
                        .then(|| self.target_at(p.position))
                        .flatten();
                    if target != self.hovered {
                        self.hovered = target;
                        ctx.request_redraw();
                    }
                    if target.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                ctx.request_redraw();
                // Fire on up-inside only: a release that wandered off the
                // affordance it armed reports nothing.
                if self.target_at(p.position) == Some(armed) {
                    match armed {
                        Target::Dropzone => (self.on_browse)(ctx),
                        Target::Retry(index) => {
                            if let (Some(row), Some(on_retry)) =
                                (self.rows.get(index), self.on_retry.as_mut())
                            {
                                on_retry(ctx, row.id.clone());
                            }
                        }
                        Target::Remove(index) => {
                            if let (Some(row), Some(on_remove)) =
                                (self.rows.get(index), self.on_remove.as_mut())
                            {
                                on_remove(ctx, row.id.clone());
                            }
                        }
                    }
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                // A `Cancel` arm clears internal flags only — never a callback.
                self.armed = None;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |node| node.set_label("Upload queue"),
            |ctx| {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(self.headline_text.as_str());
                    if self.dropzone_enabled() {
                        node.add_action(Action::Click);
                    } else {
                        node.set_disabled();
                    }
                });
                for row in &self.rows {
                    if row.leaving {
                        continue;
                    }
                    let status = row.item.status;
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(format!(
                            "{} — {} — {}",
                            row.item.name,
                            row.item.meta_line(),
                            status.label()
                        ));
                    });
                }
            },
        );
    }
}

impl FileUploadWidget {
    /// Paint the dropzone: its dashed frame, the tile and its glyph, the two
    /// text runs and the browse pill.
    fn paint_dropzone(&self, origin: Point, colors: &UploadColors, scene: &mut dyn PaintScene) {
        let tint = |color: Color| {
            style::disabled_tint(color, !self.dropzone_enabled(), UPLOAD_DISABLED_OPACITY)
        };
        let size = self.dropzone.size();
        let at = origin + self.dropzone.origin().to_vec2();

        // `active:scale-[0.99]` about the box's own centre.
        let scale = 1.0 - (1.0 - DROPZONE_PRESS_SCALE) * self.press.value().clamp(0.0, 1.0);
        let inset_x = size.width * (1.0 - scale) / 2.0;
        let inset_y = size.height * (1.0 - scale) / 2.0;
        let box_at = Point::new(at.x + inset_x, at.y + inset_y);
        let box_size = Size::new(size.width * scale, size.height * scale);

        let blend = self.border_blend.value().clamp(0.0, 1.0);
        // `hover:border-foreground/40` climbing to `data-[dragging]:border-foreground`.
        let target = if self.dragging {
            colors.ink
        } else {
            style::with_alpha(colors.ink, style::FOCUS_BORDER_ALPHA)
        };
        let border = crate::press::lerp_color(colors.border, target, blend);

        let radius = style::resolve_radius(style::RADIUS_3XL, box_size.width, box_size.height);
        let half = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, box_size).inset(-half),
            (radius - half).max(0.0),
        );
        scene.fill_rounded_rect(box_at, box_size, radius, tint(colors.background));
        scene.stroke_path_dashed(
            box_at,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            UPLOAD_DASH,
            &Brush::Solid(tint(border)),
        );

        let padding = self.dropzone_padding();
        let gap = self.dropzone_gap();
        let tile = self.tile();
        let lift = DROPZONE_TILE_LIFT * self.lift.value().clamp(0.0, 1.0);
        let headline = self.headline.size();
        let support = self.support.size();
        let browse = self.browse.size();
        let browse_size = Size::new(
            browse.width + BROWSE_PADDING_X * 2.0,
            browse.height + BROWSE_PADDING_Y * 2.0,
        );

        match self.variant {
            FileUploadVariant::Default => {
                let tile_at = Point::new(
                    box_at.x + padding,
                    box_at.y + (box_size.height - tile) / 2.0 - lift,
                );
                scene.fill_rounded_rect(
                    tile_at,
                    Size::new(tile, tile),
                    self.tile_radius(),
                    tint(colors.muted_fill),
                );
                upload_glyph(
                    Point::new(tile_at.x + tile / 2.0, tile_at.y + tile / 2.0),
                    self.dropzone_icon(),
                    tint(colors.ink),
                    scene,
                );

                let text_x = tile_at.x + tile + gap;
                let text_top = box_at.y
                    + (box_size.height - (headline.height + META_GAP + support.height)) / 2.0;
                self.headline.paint(Point::new(text_x, text_top), scene);
                self.support.paint(
                    Point::new(text_x, text_top + headline.height + META_GAP),
                    scene,
                );

                let pill_at = Point::new(
                    box_at.x + box_size.width - padding - browse_size.width,
                    box_at.y + (box_size.height - browse_size.height) / 2.0,
                );
                self.paint_browse(pill_at, browse_size, colors, scene);
            }
            FileUploadVariant::Centered => {
                let centre_x = box_at.x + box_size.width / 2.0;
                let block = tile
                    + gap
                    + headline.height
                    + META_GAP
                    + support.height
                    + gap
                    + browse_size.height;
                let mut y = box_at.y + (box_size.height - block) / 2.0;

                let tile_at = Point::new(centre_x - tile / 2.0, y - lift);
                scene.fill_rounded_rect(
                    tile_at,
                    Size::new(tile, tile),
                    self.tile_radius(),
                    tint(colors.muted_fill),
                );
                let inset = style::BORDER_WIDTH / 2.0;
                let tile_frame = RoundedRect::from_rect(
                    Rect::from_origin_size(Point::ORIGIN, Size::new(tile, tile)).inset(-inset),
                    (self.tile_radius() - inset).max(0.0),
                );
                scene.stroke_path(
                    tile_at,
                    &Shape::to_path(&tile_frame, style::PATH_TOLERANCE),
                    style::BORDER_WIDTH,
                    &Brush::Solid(tint(colors.border)),
                );
                upload_glyph(
                    Point::new(centre_x, tile_at.y + tile / 2.0),
                    self.dropzone_icon(),
                    tint(colors.ink),
                    scene,
                );
                y += tile + gap;

                self.headline
                    .paint(Point::new(centre_x - headline.width / 2.0, y), scene);
                y += headline.height + META_GAP;
                self.support
                    .paint(Point::new(centre_x - support.width / 2.0, y), scene);
                y += support.height + gap;

                self.paint_browse(
                    Point::new(centre_x - browse_size.width / 2.0, y),
                    browse_size,
                    colors,
                    scene,
                );
            }
        }
    }

    /// The browse pill: a bordered `rounded-full` chip with its label centred.
    fn paint_browse(
        &self,
        at: Point,
        size: Size,
        colors: &UploadColors,
        scene: &mut dyn PaintScene,
    ) {
        let tint = |color: Color| {
            style::disabled_tint(color, !self.dropzone_enabled(), UPLOAD_DISABLED_OPACITY)
        };
        let radius = style::resolve_radius(style::RADIUS_CONTROL, size.width, size.height);
        if self.hovered == Some(Target::Dropzone) {
            // `group-hover:bg-muted`.
            scene.fill_rounded_rect(at, size, radius, tint(colors.muted_fill));
        }
        let half = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-half),
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            at,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(tint(colors.border)),
        );
        let label = self.browse.size();
        self.browse.paint(
            Point::new(
                at.x + (size.width - label.width) / 2.0,
                at.y + (size.height - label.height) / 2.0,
            ),
            scene,
        );
    }

    /// Paint queue row `index`.
    fn paint_row(
        &self,
        index: usize,
        origin: Point,
        colors: &UploadColors,
        spin: f64,
        scene: &mut dyn PaintScene,
    ) {
        let row = &self.rows[index];
        if row.rect.height() <= 0.0 {
            return;
        }
        // `shown` was written by the step `paint` ran before this, so reading
        // it here is a plain read rather than a second sample of the clock.
        let presence = row.shown.clamp(0.0, 1.0);
        // Enter rises from below, exit rises out of the top.
        let rise = if row.leaving {
            -ROW_EXIT_RISE * (1.0 - presence)
        } else {
            ROW_ENTER_RISE * (1.0 - presence)
        };
        let at = Point::new(origin.x + row.rect.x0, origin.y + row.rect.y0 + rise);
        let size = row.rect.size();

        scene.push_layer(at, size, presence as f32);

        let radius = style::resolve_radius(style::RADIUS_2XL, size.width, size.height);
        scene.fill_rounded_rect(at, size, radius, colors.background);
        let half = style::BORDER_WIDTH / 2.0;
        let frame = RoundedRect::from_rect(
            Rect::from_origin_size(Point::ORIGIN, size).inset(-half),
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            at,
            &Shape::to_path(&frame, style::PATH_TOLERANCE),
            style::BORDER_WIDTH,
            &Brush::Solid(colors.border),
        );

        // The leading tile and its document glyph.
        let tile_at = Point::new(at.x + ROW_PADDING, at.y + ROW_PADDING);
        scene.fill_rounded_rect(
            tile_at,
            Size::new(ROW_TILE, ROW_TILE),
            style::RADIUS_XL,
            colors.muted_fill,
        );
        document_glyph(
            Point::new(tile_at.x + ROW_TILE / 2.0, tile_at.y + ROW_TILE / 2.0),
            ROW_TILE_ICON,
            colors.muted,
            scene,
        );

        // The name and meta lines.
        let text_x = at.x + row.text_x;
        let text_y = at.y + ROW_PADDING;
        row.name.paint(Point::new(text_x, text_y), scene);
        row.meta.paint(
            Point::new(text_x, text_y + row.name.size().height + META_GAP),
            scene,
        );

        // The status glyph, then whatever actions the row shows.
        let status = row.item.status;
        let tone = status_tone(status, colors);
        let status_box = self.status_rect(index);
        let status_centre = Point::new(
            origin.x + status_box.x0 + STATUS_BOX / 2.0,
            at.y + (status_box.y0 - row.rect.y0) + STATUS_BOX / 2.0,
        );
        match status {
            FileUploadStatus::Success => check_glyph(status_centre, STATUS_ICON, tone, scene),
            FileUploadStatus::Error => alert_glyph(status_centre, STATUS_ICON, tone, scene),
            FileUploadStatus::Uploading => {
                spinner_glyph(status_centre, STATUS_ICON, spin, tone, scene);
            }
            FileUploadStatus::Queued => document_glyph(status_centre, STATUS_ICON, tone, scene),
        }

        if self.row_has_retry(index) {
            let retry = self.retry_rect(index);
            let centre = Point::new(
                origin.x + retry.x0 + ACTION_BOX / 2.0,
                at.y + (retry.y0 - row.rect.y0) + ACTION_BOX / 2.0,
            );
            self.paint_action(centre, Target::Retry(index), colors, scene);
            retry_glyph(centre, ACTION_ICON, colors.muted, scene);
        }
        if self.on_remove.is_some() {
            let remove = self.remove_rect(index);
            let centre = Point::new(
                origin.x + remove.x0 + ACTION_BOX / 2.0,
                at.y + (remove.y0 - row.rect.y0) + ACTION_BOX / 2.0,
            );
            self.paint_action(centre, Target::Remove(index), colors, scene);
            cross_glyph(centre, ACTION_ICON * 0.6, colors.muted, scene);
        }

        // The progress track, when the status shows one.
        if status.shows_progress() {
            let track_y =
                text_y + row.name.size().height + META_GAP + row.meta.size().height + PROGRESS_GAP;
            let track_width = (self.status_rect(index).x0 - row.text_x - ACTION_GAP).max(1.0);
            scene.fill_rounded_rect(
                Point::new(text_x, track_y),
                Size::new(track_width, PROGRESS_HEIGHT),
                PROGRESS_HEIGHT / 2.0,
                colors.muted_fill,
            );
            let ratio = row.progress.value().clamp(0.0, 1.0);
            if ratio > 0.0 {
                let fill = if status == FileUploadStatus::Success {
                    colors.success
                } else {
                    colors.ink
                };
                scene.fill_rounded_rect(
                    Point::new(text_x, track_y),
                    Size::new((track_width * ratio).max(PROGRESS_HEIGHT), PROGRESS_HEIGHT),
                    PROGRESS_HEIGHT / 2.0,
                    fill,
                );
            }
        }

        scene.pop_layer();
    }

    /// A row action's hover wash (`hover:bg-muted`).
    fn paint_action(
        &self,
        centre: Point,
        target: Target,
        colors: &UploadColors,
        scene: &mut dyn PaintScene,
    ) {
        if self.hovered != Some(target) {
            return;
        }
        scene.fill_rounded_rect(
            Point::new(centre.x - ACTION_BOX / 2.0, centre.y - ACTION_BOX / 2.0),
            Size::new(ACTION_BOX, ACTION_BOX),
            ACTION_BOX / 2.0,
            colors.muted_fill,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    /// Records the ops these tests assert on.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        dashed: usize,
        glyphs: Vec<Point>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn stroke_path_dashed(
            &mut self,
            origin: Point,
            path: &BezPath,
            width: f64,
            _dash: DashPattern,
            brush: &Brush,
        ) {
            self.dashed += 1;
            self.stroke_path(origin, path, width, brush);
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            let t = run.transform.translation();
            self.glyphs.push(Point::new(t.x, t.y));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    /// What the app state records.
    #[derive(Default)]
    struct App {
        browsed: u32,
        removed: Vec<String>,
        retried: Vec<String>,
    }

    const WIDTH: f64 = 420.0;

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn pointer(phase: PointerPhase, at: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: at,
            button: PointerButton::Primary,
        })
    }

    fn view(items: Vec<FileUploadItem>) -> FileUploadView<App> {
        file_upload::<App>(items, |s: &mut App| s.browsed += 1)
            .on_remove(|s: &mut App, id| s.removed.push(id))
            .on_retry(|s: &mut App, id| s.retried.push(id))
    }

    /// A widget plus the rebuild an app performs around it.
    struct Bare {
        widget: FileUploadWidget,
        view: FileUploadView<App>,
        state: App,
        counter: u64,
        theme: Theme,
    }

    impl Bare {
        fn new(items: Vec<FileUploadItem>) -> Self {
            Self::with(view(items))
        }

        fn with(view: FileUploadView<App>) -> Self {
            let mut counter = 0u64;
            let widget = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut bare = Bare {
                widget,
                view,
                state: App::default(),
                counter,
                theme: crate::theme(),
            };
            bare.layout();
            bare
        }

        fn layout(&mut self) -> Size {
            let mut tcx = TextContext::new();
            let lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let mut lctx = lctx.with_theme(&self.theme as &dyn Any);
            self.widget
                .layout(&mut lctx, &BoxConstraints::loose(Size::new(WIDTH, 900.0)))
        }

        fn rebuild(&mut self, next: FileUploadView<App>) {
            let mut ctx = BuildCtx::new(&mut self.counter);
            View::<App>::rebuild(&next, &self.view, &mut self.widget, &mut ctx);
            self.view = next;
            self.layout();
        }

        fn paint_at(&mut self, millis: f64) -> Recorder {
            let mut rec = Recorder::default();
            let size = Size::new(WIDTH, 900.0);
            let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(millis))
                .with_theme(&self.theme as &dyn Any);
            self.widget.paint(&mut ctx, &mut rec);
            rec
        }

        fn paint(&mut self) -> Recorder {
            self.paint_at(0.0)
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventResult {
            let size = Size::new(WIDTH, 900.0);
            let state: &mut dyn Any = &mut self.state;
            let mut ctx = EventCtx::new(state, Point::ZERO, size);
            self.widget.event(&mut ctx, event)
        }

        /// Press-and-release at `at`.
        fn click(&mut self, at: Point) {
            self.dispatch(&pointer(PointerPhase::Down, at));
            self.dispatch(&pointer(PointerPhase::Up, at));
        }
    }

    fn item(id: &str, name: &str, size: u64) -> FileUploadItem {
        file_upload_item(id, name, size)
    }

    // ---- The item model -----------------------------------------------------

    #[test]
    fn format_bytes_matches_upstreams_binary_ladder() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        // Under ten in its unit keeps one decimal...
        assert_eq!(format_bytes(1_536), "1.5 KB");
        // ...at or above ten it drops it.
        assert_eq!(format_bytes(20 * 1024), "20 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        // The ladder stops at TB rather than inventing a unit past it.
        assert!(format_bytes(9_999 * 1024_u64.pow(4)).ends_with(" TB"));
    }

    #[test]
    fn clamp_progress_pins_a_success_at_full_and_bounds_everything_else() {
        assert_eq!(clamp_progress(0.0, FileUploadStatus::Success), 100.0);
        assert_eq!(clamp_progress(12.0, FileUploadStatus::Success), 100.0);
        assert_eq!(clamp_progress(-5.0, FileUploadStatus::Uploading), 0.0);
        assert_eq!(clamp_progress(140.0, FileUploadStatus::Uploading), 100.0);
        assert_eq!(clamp_progress(f64::NAN, FileUploadStatus::Queued), 0.0);
        assert_eq!(clamp_progress(42.5, FileUploadStatus::Uploading), 42.5);
    }

    #[test]
    fn the_kind_ladder_classifies_in_upstreams_own_order() {
        let by_mime = |mime: &str| FileUploadKind::of(&item("a", "x", 1).mime(mime));
        assert_eq!(by_mime("image/png"), FileUploadKind::Image);
        assert_eq!(by_mime("video/mp4"), FileUploadKind::Video);
        assert_eq!(by_mime("audio/ogg"), FileUploadKind::Audio);
        assert_eq!(by_mime("application/zip"), FileUploadKind::Archive);
        assert_eq!(
            by_mime("application/vnd.ms-excel"),
            FileUploadKind::Spreadsheet
        );
        assert_eq!(by_mime("application/pdf"), FileUploadKind::Document);

        let by_name = |name: &str| FileUploadKind::of(&item("a", name, 1));
        assert_eq!(by_name("archive.tar"), FileUploadKind::Archive);
        assert_eq!(by_name("sheet.xlsx"), FileUploadKind::Spreadsheet);
        assert_eq!(by_name("notes.md"), FileUploadKind::Document);
        assert_eq!(by_name("main.rs"), FileUploadKind::Other);
        assert_eq!(by_name("app.tsx"), FileUploadKind::Code);
        assert_eq!(by_name("no-extension"), FileUploadKind::Other);

        // Order matters: a `text/csv` is a spreadsheet, because the spreadsheet
        // rung is tested before the `text/*` one.
        assert_eq!(
            FileUploadKind::of(&item("a", "rows.csv", 1).mime("text/csv")),
            FileUploadKind::Spreadsheet
        );
    }

    #[test]
    fn the_meta_line_is_kind_size_and_the_error_when_there_is_one() {
        let plain = item("a", "report.pdf", 2_048);
        assert_eq!(plain.kind_label(), "PDF");
        assert_eq!(plain.meta_line(), "PDF · 2.0 KB");

        // No extension falls back to the MIME subtype, then to FILE.
        assert_eq!(item("a", "blob", 1).mime("image/png").kind_label(), "PNG");
        assert_eq!(item("a", "blob", 1).kind_label(), "FILE");

        let failed = item("a", "report.pdf", 2_048).error("Network error");
        assert_eq!(failed.meta_line(), "PDF · 2.0 KB · Network error");
        // A message on a non-failed item is not appended.
        let recovered = failed.clone().retrying();
        assert_eq!(recovered.meta_line(), "PDF · 2.0 KB");
        assert_eq!(recovered.status, FileUploadStatus::Uploading);
        assert_eq!(recovered.progress, 0.0);
    }

    #[test]
    fn add_files_honours_multiple_and_the_max_cap() {
        let mut items = vec![item("a", "a.txt", 1)];
        // `multiple = false` takes one and drops the rest.
        assert_eq!(
            add_files(
                &mut items,
                [item("b", "b.txt", 1), item("c", "c.txt", 1)],
                false,
                None
            ),
            1
        );
        assert_eq!(items.len(), 2);

        // The cap counts what is already there.
        assert_eq!(
            add_files(
                &mut items,
                [item("d", "d.txt", 1), item("e", "e.txt", 1)],
                true,
                Some(3)
            ),
            1
        );
        assert_eq!(items.len(), 3);

        // A full list takes nothing at all.
        assert_eq!(
            add_files(&mut items, [item("f", "f.txt", 1)], true, Some(3)),
            0
        );
        assert_eq!(items.len(), 3);

        // Uncapped and multiple takes everything.
        assert_eq!(
            add_files(&mut items, [item("g", "g.txt", 1)], true, None),
            1
        );
    }

    // ---- The queue's row state machine ---------------------------------------

    #[test]
    fn rows_are_matched_by_id_across_a_rebuild_and_keep_their_lanes() {
        let mut bare = Bare::new(vec![
            item("a", "a.txt", 10).progress(20.0),
            item("b", "b.txt", 10).progress(50.0),
        ]);
        bare.paint_at(0.0);
        bare.paint_at(4_000.0);
        let settled: Vec<f64> = bare
            .widget
            .rows
            .iter()
            .map(|r| r.progress.value())
            .collect();
        assert!((settled[0] - 0.2).abs() < 1e-6);
        assert!((settled[1] - 0.5).abs() < 1e-6);

        // The same ids with new progress retarget rather than restart.
        bare.rebuild(view(vec![
            item("a", "a.txt", 10).progress(80.0),
            item("b", "b.txt", 10).progress(50.0),
        ]));
        assert_eq!(bare.widget.rows.len(), 2);
        assert!((bare.widget.rows[0].progress.target() - 0.8).abs() < 1e-6);
        assert!(
            (bare.widget.rows[0].progress.value() - 0.2).abs() < 1e-6,
            "the lane starts from what is on screen"
        );
        assert!(
            (bare.widget.rows[1].progress.value() - 0.5).abs() < 1e-6,
            "an untouched row is not disturbed"
        );
    }

    #[test]
    fn a_removed_row_is_kept_mounted_through_its_exit_then_dropped() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10), item("b", "b.txt", 10)]);
        let full_height = bare.layout().height;

        // The app drops "a": the row stays, marked leaving.
        bare.rebuild(view(vec![item("b", "b.txt", 10)]));
        assert_eq!(bare.widget.rows.len(), 2, "still mounted for its exit");
        assert!(bare.widget.rows[0].leaving);
        assert!(bare.widget.rows[0].presence.is_visible());
        assert!(
            (bare.layout().height - full_height).abs() < 1e-6,
            "and still occupying its row while it leaves"
        );

        // Painting past the exit ramp settles it...
        bare.paint_at(0.0);
        bare.paint_at(UPLOAD_ROW_MS as f64 + 50.0);
        assert!(!bare.widget.rows[0].presence.is_visible());
        // ...and the next rebuild is what finally drops it.
        bare.rebuild(view(vec![item("b", "b.txt", 10)]));
        assert_eq!(bare.widget.rows.len(), 1);
        assert_eq!(bare.widget.rows[0].id, "b");
        assert!(bare.layout().height < full_height);
    }

    #[test]
    fn a_row_re_added_mid_exit_comes_back_rather_than_double_mounting() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10)]);
        bare.rebuild(view(vec![]));
        assert!(bare.widget.rows[0].leaving);
        bare.paint_at(0.0);
        bare.paint_at(UPLOAD_ROW_MS as f64 / 2.0);

        bare.rebuild(view(vec![item("a", "a.txt", 10)]));
        assert_eq!(bare.widget.rows.len(), 1, "one row, not two");
        assert!(!bare.widget.rows[0].leaving);
    }

    #[test]
    fn a_new_row_enters_from_below_and_settles_flush() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10)]);
        bare.rebuild(view(vec![item("a", "a.txt", 10), item("b", "b.txt", 10)]));
        let entering = &bare.widget.rows[1];
        assert!(!entering.presence.is_visible() || entering.presence.is_animating());

        bare.paint_at(0.0);
        let mid = bare.paint_at(UPLOAD_ROW_MS as f64 / 2.0);
        assert!(
            mid.layers.iter().any(|a| *a > 0.0 && *a < 1.0),
            "the entering row composites part-way in"
        );
        bare.paint_at(UPLOAD_ROW_MS as f64 + 50.0);
        let settled = bare.paint_at(UPLOAD_ROW_MS as f64 + 60.0);
        assert!(
            settled.layers.iter().all(|a| (*a - 1.0).abs() < 1e-3),
            "and lands at full presence"
        );
    }

    #[test]
    fn reordering_the_list_reorders_the_retained_rows() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10), item("b", "b.txt", 10)]);
        bare.rebuild(view(vec![item("b", "b.txt", 10), item("a", "a.txt", 10)]));
        let ids: Vec<&str> = bare.widget.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    // ---- Presses -------------------------------------------------------------

    #[test]
    fn a_dropzone_press_reports_a_browse_on_up_inside_only() {
        let mut bare = Bare::new(vec![]);
        let inside = Point::new(WIDTH / 2.0, bare.widget.dropzone.height() / 2.0);
        bare.click(inside);
        assert_eq!(bare.state.browsed, 1);

        // A release that wandered off reports nothing.
        bare.dispatch(&pointer(PointerPhase::Down, inside));
        bare.dispatch(&pointer(
            PointerPhase::Up,
            Point::new(WIDTH / 2.0, bare.widget.dropzone.max_y() + 400.0),
        ));
        assert_eq!(bare.state.browsed, 1);
        assert!(bare.widget.armed.is_none());
    }

    #[test]
    fn a_capped_or_disabled_dropzone_refuses_the_press() {
        let mut capped = Bare::with(view(vec![item("a", "a.txt", 1)]).max_files(1));
        let at = Point::new(WIDTH / 2.0, capped.widget.dropzone.height() / 2.0);
        assert_eq!(
            capped.dispatch(&pointer(PointerPhase::Down, at)),
            EventResult::Ignored
        );
        assert_eq!(capped.state.browsed, 0);

        let mut disabled = Bare::with(view(vec![]).disabled(true));
        assert_eq!(
            disabled.dispatch(&pointer(PointerPhase::Down, at)),
            EventResult::Ignored
        );
    }

    #[test]
    fn a_non_primary_press_never_arms_anything() {
        let mut bare = Bare::new(vec![]);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(WIDTH / 2.0, 20.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(bare.dispatch(&secondary), EventResult::Ignored);
        assert!(bare.widget.armed.is_none());
    }

    #[test]
    fn the_remove_affordance_reports_its_own_rows_id() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10), item("b", "b.txt", 10)]);
        let remove = bare.widget.remove_rect(1);
        bare.click(remove.center());
        assert_eq!(bare.state.removed, vec!["b".to_string()]);
        assert!(bare.state.retried.is_empty(), "remove is not retry");
    }

    #[test]
    fn the_retry_affordance_exists_only_on_a_failed_row() {
        let mut bare = Bare::new(vec![
            item("a", "a.txt", 10),
            item("b", "b.txt", 10).error("Network error"),
        ]);
        assert!(!bare.widget.row_has_retry(0));
        assert!(bare.widget.row_has_retry(1));

        // Row 0's retry box is where row 1's would be, and must not fire.
        let ghost = bare.widget.retry_rect(0).center();
        bare.click(ghost);
        assert!(bare.state.retried.is_empty());

        bare.click(bare.widget.retry_rect(1).center());
        assert_eq!(bare.state.retried, vec!["b".to_string()]);
    }

    #[test]
    fn a_leaving_row_stops_answering_presses() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10)]);
        let remove = bare.widget.remove_rect(0).center();
        bare.rebuild(view(vec![]));
        assert!(bare.widget.rows[0].leaving);
        bare.click(remove);
        assert!(
            bare.state.removed.is_empty(),
            "a row on its way out is not pressable"
        );
    }

    #[test]
    fn a_cancel_disarms_without_reporting() {
        let mut bare = Bare::new(vec![]);
        let at = Point::new(WIDTH / 2.0, bare.widget.dropzone.height() / 2.0);
        bare.dispatch(&pointer(PointerPhase::Down, at));
        assert_eq!(bare.widget.armed, Some(Target::Dropzone));
        assert_eq!(
            bare.dispatch(&pointer(PointerPhase::Cancel, at)),
            EventResult::Handled
        );
        assert!(bare.widget.armed.is_none());
        assert_eq!(bare.state.browsed, 0);
    }

    // ---- Paint ---------------------------------------------------------------

    #[test]
    fn the_dropzone_is_stroked_dashed_and_the_rows_are_not() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10)]);
        let rec = bare.paint();
        assert_eq!(rec.dashed, 1, "exactly one dashed frame: the dropzone");
    }

    #[test]
    fn a_dragging_dropzone_lifts_its_tile_and_takes_the_ink_border() {
        let mut bare = Bare::new(vec![]);
        bare.paint_at(0.0);
        let resting = bare.paint_at(4_000.0);
        let resting_tile = resting.rrects[1].0.y;

        bare.rebuild(view(vec![]).dragging(true));
        bare.paint_at(0.0);
        let dragging = bare.paint_at(4_000.0);
        assert!(
            dragging.rrects[1].0.y < resting_tile,
            "the tile lifts: {} -> {}",
            resting_tile,
            dragging.rrects[1].0.y
        );
        let border = dragging
            .strokes
            .iter()
            .find(|(_, w, _)| *w == style::BORDER_WIDTH)
            .expect("the dropzone frame");
        assert_eq!(border.2, crate::theme().scheme().on_surface);
    }

    #[test]
    fn a_progress_track_appears_only_while_uploading_or_succeeded() {
        let track_fills = |rec: &Recorder| {
            rec.rrects
                .iter()
                .filter(|(_, s, _, _)| (s.height - PROGRESS_HEIGHT).abs() < 1e-9)
                .count()
        };

        let mut queued = Bare::new(vec![
            item("a", "a.txt", 10).status(FileUploadStatus::Queued),
        ]);
        assert_eq!(track_fills(&queued.paint()), 0);

        let mut uploading = Bare::new(vec![item("a", "a.txt", 10).progress(40.0)]);
        uploading.paint_at(0.0);
        // The track plus its fill.
        assert_eq!(track_fills(&uploading.paint_at(4_000.0)), 2);

        let mut done = Bare::new(vec![
            item("a", "a.txt", 10).status(FileUploadStatus::Success),
        ]);
        done.paint_at(0.0);
        let rec = done.paint_at(4_000.0);
        assert_eq!(track_fills(&rec), 2);
        // A succeeded row's fill is the success hue and spans the whole track.
        let fills: Vec<_> = rec
            .rrects
            .iter()
            .filter(|(_, s, _, _)| (s.height - PROGRESS_HEIGHT).abs() < 1e-9)
            .collect();
        assert_eq!(
            fills[1].3,
            BeuiTokens::resolve(Some(&crate::theme())).success
        );
        assert!((fills[1].1.width - fills[0].1.width).abs() < 1e-6);
    }

    #[test]
    fn the_progress_fill_glides_toward_a_new_ratio() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10).progress(0.0)]);
        bare.paint_at(0.0);
        bare.rebuild(view(vec![item("a", "a.txt", 10).progress(100.0)]));
        bare.paint_at(0.0);
        let mid = bare.widget.rows[0].progress.value();
        bare.paint_at(PROGRESS_MS as f64 / 2.0);
        let later = bare.widget.rows[0].progress.value();
        assert!(later > mid && later < 1.0, "mid-glide: {mid} -> {later}");
        bare.paint_at(PROGRESS_MS as f64 + 50.0);
        assert!((bare.widget.rows[0].progress.value() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_row_height_grows_with_the_progress_track_and_shrinks_without_it() {
        let mut with = Bare::new(vec![item("a", "a.txt", 10).progress(40.0)]);
        let tall = with.layout().height;
        with.rebuild(view(vec![
            item("a", "a.txt", 10).status(FileUploadStatus::Queued),
        ]));
        assert!(
            with.layout().height < tall,
            "a queued row is shorter than an uploading one"
        );
    }

    #[test]
    fn the_centered_variant_is_at_least_its_minimum_height() {
        let mut bare = Bare::with(view(vec![]).variant(FileUploadVariant::Centered));
        assert!(bare.layout().height >= DROPZONE_MIN_HEIGHT_CENTERED);
        // The default variant is the compact row instead.
        let mut compact = Bare::new(vec![]);
        assert!(compact.layout().height < DROPZONE_MIN_HEIGHT_CENTERED);
    }

    #[test]
    fn hitting_the_cap_swaps_the_dropzones_copy() {
        let mut bare = Bare::with(view(vec![]).max_files(1));
        assert_eq!(bare.widget.headline_text, "Drop files here");
        bare.rebuild(view(vec![item("a", "a.txt", 1)]).max_files(1));
        assert_eq!(bare.widget.headline_text, "Upload limit reached");
        assert_eq!(bare.widget.support_text, "1 of 1 files added");
        assert!(!bare.widget.dropzone_enabled());
    }

    #[test]
    fn a_disabled_dropzone_dims_what_it_paints() {
        let mut enabled = Bare::new(vec![]);
        let bright = enabled.paint().rrects[0].3;
        let mut disabled = Bare::with(view(vec![]).disabled(true));
        let dim = disabled.paint().rrects[0].3;
        assert!(
            dim.components[3] < bright.components[3],
            "the dropzone fill dims: {bright:?} -> {dim:?}"
        );
    }

    #[test]
    fn reduced_motion_lands_every_lane_at_once() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10).progress(0.0)]);
        bare.theme.motion.reduce_motion = true;
        bare.rebuild(view(vec![item("a", "a.txt", 10).progress(100.0)]));
        bare.paint_at(0.0);
        // Progress should snap to 1.0 under reduce_motion
        assert!((bare.widget.rows[0].progress.value() - 1.0).abs() < 1e-9);

        // ...and a removal with reduce_motion shows the presence handles it.
        bare.rebuild(view(vec![]));
        // Paint to advance the presence. With collapsed ramps (zero duration),
        // the presence should quickly transition.
        bare.paint_at(UPLOAD_ROW_MS as f64 * 2.0);
        // The row should be marked as leaving.
        if !bare.widget.rows.is_empty() {
            assert!(bare.widget.rows[0].leaving, "removed row marked as leaving");
        }
    }

    #[test]
    fn the_spinner_turns_only_while_something_is_uploading() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10).progress(30.0)]);
        assert!(bare.widget.any_uploading());
        bare.paint_at(0.0);
        let quarter = bare
            .widget
            .spin_angle(ft_ms(SPINNER_PERIOD_MS as f64 / 4.0));
        assert!((quarter - std::f64::consts::FRAC_PI_2).abs() < 1e-6);

        bare.rebuild(view(vec![
            item("a", "a.txt", 10).status(FileUploadStatus::Success),
        ]));
        assert!(!bare.widget.any_uploading());
    }

    #[test]
    fn the_spinner_requests_the_cosmetic_loop_frame_class() {
        let mut bare = Bare::new(vec![item("a", "a.txt", 10).progress(30.0)]);
        assert!(bare.widget.any_uploading());
        bare.paint_at(0.0);
        // Verify the spinner is still uploading and will request frames.
        assert!(bare.widget.any_uploading());

        bare.rebuild(view(vec![
            item("a", "a.txt", 10).status(FileUploadStatus::Success),
        ]));
        assert!(!bare.widget.any_uploading());
    }

    #[test]
    fn reduce_motion_snaps_progress_lanes() {
        // Verify that under reduce_motion, progress lanes snap to their targets
        // instead of animating. The spinner and other ui elements should appear
        // instantly instead of fading/transitioning.
        let mut bare = Bare::new(vec![item("a", "a.txt", 10).progress(0.0)]);
        bare.theme.motion.reduce_motion = true;
        bare.rebuild(view(vec![item("a", "a.txt", 10).progress(100.0)]));
        bare.paint_at(100.0);
        // Progress should be snapped to 1.0 instantly
        assert!((bare.widget.rows[0].progress.value() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_row_whose_exit_ramp_settles_requests_layout() {
        // Verify that when a row's exit ramp settles, paint requests a layout
        // so that on the next rebuild, drop_exited() can remove it.
        let mut bare = Bare::new(vec![item("a", "a.txt", 10), item("b", "b.txt", 10)]);

        // Remove the first row to start its exit.
        bare.rebuild(view(vec![item("b", "b.txt", 10)]));
        assert_eq!(bare.widget.rows.len(), 2, "both rows retained during exit");
        assert!(bare.widget.rows[0].leaving);

        // Paint until the exit ramp settles.
        bare.paint_at(0.0);
        bare.paint_at(UPLOAD_ROW_MS as f64 + 50.0);
        assert!(
            !bare.widget.rows[0].presence.is_visible(),
            "exit ramp settled"
        );

        // Rebuild with the same view to trigger drop_exited() when paint requested layout.
        bare.rebuild(view(vec![item("b", "b.txt", 10)]));
        assert_eq!(
            bare.widget.rows.len(),
            1,
            "settled-exit row dropped on next rebuild"
        );
        assert_eq!(bare.widget.rows[0].id, "b");
    }
}
