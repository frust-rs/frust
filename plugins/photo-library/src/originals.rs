//! Exporting an asset's original file, and saving a file into the photo library.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cancel::{CancelToken, ProgressThrottle};
use crate::media_types::classify_path;
use crate::{AssetId, MediaKind, PhotoError, PickProgress};

/// Which rendition of an asset to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum AssetVersion {
    /// The file as first captured or imported (the default).
    #[default]
    Original,
    /// The edited rendition when the asset has edits; the original otherwise.
    Current,
}

/// Receives export progress as a fraction in `0.0..=1.0`. May run on any thread.
pub type ProgressFn = Arc<dyn Fn(f64) + Send + Sync>;

/// Options for [`PhotoLibrary::export_original`](crate::PhotoLibrary::export_original).
#[derive(Clone)]
#[non_exhaustive]
pub struct ExportOptions {
    /// Directory the file is written into. `None` uses
    /// `<cache_dir>/frust-photo-library/originals/`, which the app owns and cleans up
    /// ([`PhotoLibrary::clear_pick_cache`](crate::PhotoLibrary::clear_pick_cache) leaves it alone).
    pub dest_dir: Option<PathBuf>,
    /// Which rendition to export.
    pub version: AssetVersion,
    /// Whether a cloud-only original may be downloaded (default `true`). When `false` such an
    /// asset fails with [`PhotoError::NetworkRequired`].
    pub allow_network: bool,
    /// Progress callback, rate-limited and monotonic; the final `1.0` is delivered on success.
    pub progress: Option<ProgressFn>,
    /// Lets the caller abort the export. A cancelled export leaves no partial file.
    pub cancel: Option<CancelToken>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            dest_dir: None,
            version: AssetVersion::default(),
            allow_network: true,
            progress: None,
            cancel: None,
        }
    }
}

impl ExportOptions {
    /// Original rendition, default directory, network allowed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the destination directory.
    pub fn dest_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dest_dir = Some(dir.into());
        self
    }

    /// Sets the rendition to export.
    pub fn version(mut self, version: AssetVersion) -> Self {
        self.version = version;
        self
    }

    /// Sets whether a cloud-only original may be downloaded.
    pub fn allow_network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }

    /// Reports progress as a fraction in `0.0..=1.0`.
    pub fn on_progress(mut self, f: impl Fn(f64) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(f));
        self
    }

    /// Lets the caller cancel the export through `token`.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }
}

impl fmt::Debug for ExportOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExportOptions")
            .field("dest_dir", &self.dest_dir.is_some())
            .field("version", &self.version)
            .field("allow_network", &self.allow_network)
            .field("progress", &self.progress.is_some())
            .field("cancel", &self.cancel.is_some())
            .finish()
    }
}

/// A file written by [`PhotoLibrary::export_original`](crate::PhotoLibrary::export_original).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedOriginal {
    /// The exported file.
    pub path: PathBuf,
    /// The name the platform reported for the file, when it reports one. Untrusted: for display
    /// only, never a path component.
    pub file_name: Option<String>,
    /// The MIME type when known.
    pub mime: Option<String>,
    /// Image or video.
    pub kind: MediaKind,
    /// Size of the file in bytes.
    pub byte_len: u64,
}

/// What to save with [`PhotoLibrary::save`](crate::PhotoLibrary::save).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SaveRequest {
    /// The file to save. It must exist and stay in place until the call returns.
    pub source: PathBuf,
    /// Image or video; inferred from the file extension when `None`.
    pub kind: Option<MediaKind>,
    /// Album to add the saved asset to, created when missing.
    pub album: Option<String>,
    /// Name for the saved asset; the source's file name when `None`. A bare name, never a path.
    pub file_name: Option<String>,
}

impl SaveRequest {
    /// Saves `source`, inferring the kind from its extension.
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            kind: None,
            album: None,
            file_name: None,
        }
    }

    /// Sets the media kind instead of inferring it.
    pub fn kind(mut self, kind: MediaKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Adds the saved asset to the album called `name`.
    pub fn album(mut self, name: impl Into<String>) -> Self {
        self.album = Some(name.into());
        self
    }

    /// Names the saved asset.
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }
}

/// The asset created by [`PhotoLibrary::save`](crate::PhotoLibrary::save).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SavedAsset {
    /// The new asset's identifier when the platform reports one.
    pub id: Option<AssetId>,
}

/// `<cache_dir>/frust-photo-library/originals`.
#[allow(dead_code)] // Used by the mobile arms.
pub(crate) fn default_dest_dir() -> Result<PathBuf, PhotoError> {
    let base = frust_paths::cache_dir().ok_or(PhotoError::PlatformNotInitialized)?;
    Ok(base.join("frust-photo-library").join("originals"))
}

/// Whether the options' token is already cancelled.
#[cfg_attr(not(feature = "library"), allow(dead_code))]
pub(crate) fn is_cancelled(options: &ExportOptions) -> bool {
    options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
}

/// Wraps an export [`ProgressFn`] in the shared throttle, so it gets a clamped, monotonic,
/// at-most-20-per-second stream whose first and final deliveries always arrive.
#[allow(dead_code)] // Used by the mobile arms.
pub(crate) fn throttled(f: ProgressFn) -> ProgressThrottle {
    ProgressThrottle::new(Arc::new(move |p: PickProgress| f(p.fraction)))
}

/// Reports one fraction through a throttle built by [`throttled`]. The single item counts as
/// done only at `1.0`, which is the throttle's final delivery, so call it with `1.0` only on
/// success.
#[allow(dead_code)] // Used by the mobile arms.
pub(crate) fn report_fraction(throttle: &ProgressThrottle, fraction: f64) {
    let done = usize::from(fraction >= 1.0);
    throttle.report(PickProgress {
        items: 1,
        done,
        fraction,
    });
}

/// Checks a save request before any platform call and resolves its media kind.
#[allow(dead_code)] // Used by `PhotoLibrary::save` under the `save` feature.
pub(crate) fn validate_save(request: &SaveRequest) -> Result<MediaKind, PhotoError> {
    if !request.source.is_file() {
        return Err(PhotoError::NotFound);
    }
    if let Some(name) = &request.file_name
        && !is_bare_name(name)
    {
        return Err(PhotoError::Platform(
            "save file name must be a bare file name".into(),
        ));
    }
    match request.kind {
        Some(kind) => Ok(kind),
        None => infer_kind(&request.source).ok_or_else(|| {
            PhotoError::Platform("cannot tell whether the file is an image or a video".into())
        }),
    }
}

#[allow(dead_code)] // Used by `validate_save`.
fn infer_kind(path: &Path) -> Option<MediaKind> {
    classify_path(path).map(|(kind, _)| kind)
}

#[allow(dead_code)] // Used by `validate_save`.
fn is_bare_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frust-photo-originals-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn export_option_defaults_and_builders() {
        let o = ExportOptions::new();
        assert!(o.allow_network);
        assert_eq!(o.version, AssetVersion::Original);
        assert!(o.dest_dir.is_none() && o.progress.is_none() && o.cancel.is_none());
        let o = o
            .dest_dir("/x")
            .version(AssetVersion::Current)
            .allow_network(false)
            .on_progress(|_| {})
            .cancel(CancelToken::new());
        assert_eq!(
            format!("{o:?}"),
            "ExportOptions { dest_dir: true, version: Current, allow_network: false, progress: true, cancel: true }"
        );
    }

    #[test]
    fn cancelled_option_is_detected() {
        let t = CancelToken::new();
        let o = ExportOptions::new().cancel(t.clone());
        assert!(!is_cancelled(&o));
        t.cancel();
        assert!(is_cancelled(&o));
        assert!(!is_cancelled(&ExportOptions::new()));
    }

    #[test]
    fn token_hook_fires_once_whether_set_before_or_after_cancel() {
        let before = CancelToken::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        before.set_hook(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        before.cancel();
        before.cancel();
        assert_eq!(n.load(Ordering::SeqCst), 1);

        let after = CancelToken::new();
        after.cancel();
        let n3 = n.clone();
        after.set_hook(move || {
            n3.fetch_add(1, Ordering::SeqCst);
        });
        after.cancel();
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn progress_adapter_forwards_fractions_and_always_delivers_the_final_one() {
        let seen: Arc<Mutex<Vec<f64>>> = Arc::default();
        let s = seen.clone();
        let throttle = throttled(Arc::new(move |f| s.lock().unwrap().push(f)));
        report_fraction(&throttle, 0.0);
        report_fraction(&throttle, 0.4);
        report_fraction(&throttle, 0.2);
        report_fraction(&throttle, 1.0);
        report_fraction(&throttle, 1.0);
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.first(), Some(&0.0));
        assert_eq!(got.last(), Some(&1.0));
        assert_eq!(got.iter().filter(|f| **f == 1.0).count(), 1);
        assert!(got.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn save_validation() {
        let missing = SaveRequest::new("/nonexistent/frust/photo.jpg");
        assert_eq!(validate_save(&missing), Err(PhotoError::NotFound));

        let jpg = temp_file("a.JPG");
        assert_eq!(validate_save(&SaveRequest::new(&jpg)), Ok(MediaKind::Image));
        let mov = temp_file("b.mov");
        assert_eq!(validate_save(&SaveRequest::new(&mov)), Ok(MediaKind::Video));
        assert_eq!(
            validate_save(&SaveRequest::new(&jpg).kind(MediaKind::Video)),
            Ok(MediaKind::Video)
        );

        let unknown = temp_file("c.xyz");
        assert!(matches!(
            validate_save(&SaveRequest::new(&unknown)),
            Err(PhotoError::Platform(_))
        ));
        assert_eq!(
            validate_save(&SaveRequest::new(&unknown).kind(MediaKind::Image)),
            Ok(MediaKind::Image)
        );
        for bad in ["", "..", "a/b.jpg", "a\\b.jpg"] {
            assert!(
                matches!(
                    validate_save(&SaveRequest::new(&jpg).file_name(bad)),
                    Err(PhotoError::Platform(_))
                ),
                "{bad:?}"
            );
        }
        assert_eq!(
            validate_save(&SaveRequest::new(&jpg).file_name("ok.jpg").album("Trip")),
            Ok(MediaKind::Image)
        );
    }

    #[test]
    fn default_dest_dir_is_a_sibling_of_picked() {
        if let Ok(dir) = default_dest_dir() {
            assert!(dir.ends_with("frust-photo-library/originals"));
        }
    }
}
