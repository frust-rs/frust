//! Persistable folder access: [`FolderGrant`], its listing options and entries.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};

use crate::backend::{self, GrantState};
use crate::export::{copy_file, io_error};
use crate::{CancelToken, FileDialogError, FileFilter};

const DEFAULT_MAX_DEPTH: u32 = 8;
const DEFAULT_MAX_ENTRIES: u32 = 10_000;
const MAX_UNIQUE_NAME_ATTEMPTS: u32 = 1024;

/// Access to a user-chosen folder, from
/// [`FileDialog::pick_folder`](crate::FileDialog::pick_folder) or
/// [`FolderGrant::restore`].
///
/// Listing and copying block; on Android and iOS they refuse the UI thread
/// (see the crate-level blocking contract).
///
/// On iOS the grant holds the folder's security scope for as long as it lives,
/// and the [`FolderEntry`] values it lists are only readable while it does: keep
/// the grant alive for as long as its entries are in use.
pub struct FolderGrant {
    pub(crate) token: String,
    pub(crate) local_path: Option<PathBuf>,
    pub(crate) state: GrantState,
}

const _: () = {
    const fn assert_movable_to_a_worker<T: Send + Sync>() {}
    assert_movable_to_a_worker::<FolderGrant>();
};

impl std::fmt::Debug for FolderGrant {
    /// The token and path identify user files, so neither is printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderGrant").finish_non_exhaustive()
    }
}

impl FolderGrant {
    /// An opaque string to persist and hand back to [`FolderGrant::restore`].
    ///
    /// It can differ after a restore, so persist the restored grant's token
    /// again. It identifies user files: treat it as sensitive.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Re-acquires a folder from a persisted [`token`](FolderGrant::token).
    ///
    /// [`FileDialogError::AccessLost`] means the token is no longer honoured
    /// (the folder is gone or the grant was revoked): ask the user again.
    pub fn restore(token: &str) -> Result<FolderGrant, FileDialogError> {
        backend::restore_folder(token)
    }

    /// The folder as a filesystem path, when the platform has one. Always
    /// `None` on Android, where entries are content URIs.
    pub fn local_path(&self) -> Option<&Path> {
        self.local_path.as_deref()
    }

    /// Lists the regular files in the folder, sorted by relative path. Links
    /// are neither followed nor listed. Blocks.
    pub fn list(&self, options: &ListOptions) -> Result<Vec<FolderEntry>, FileDialogError> {
        backend::list_folder(self, &options.normalized())
    }

    /// Looks up one file by its path from the folder: the same `/`-separated string a
    /// listing puts in [`FolderEntry::relative_path`]. Blocks.
    ///
    /// Returns the file's entry, readable through [`FolderEntry::read_to_vec`] and
    /// [`FolderEntry::copy_to`] like a listed one, or `Ok(None)` when there is no regular
    /// file at that path: nothing there, a folder, a link at any segment (links are neither
    /// followed nor listed), or a hidden segment (a name starting with `.`, which a listing
    /// skips by default). Matching is exact and case-sensitive, also on a filesystem that
    /// is not.
    ///
    /// A path a listing cannot produce is refused with [`FileDialogError::Io`] before any
    /// platform call: empty, starting with `/`, holding `\` or NUL, or with a segment that is
    /// empty, `.` or `..`. [`FileDialogError::AccessLost`] means the grant is no longer
    /// honoured, as for [`FolderGrant::list`]; like it, this refuses the Android and iOS UI
    /// thread with [`FileDialogError::UiThread`].
    ///
    /// On Android each segment of the path costs one listing of a folder, and where a folder
    /// holds two documents with the same display name, the first in name order wins.
    pub fn entry(&self, relative_path: &str) -> Result<Option<FolderEntry>, FileDialogError> {
        match entry_segments(relative_path)? {
            Some(segments) => backend::resolve_entry(self, &segments),
            None => Ok(None),
        }
    }

    /// Gives up the access this grant holds.
    ///
    /// On iOS this leaves the security scope. Every [`FolderEntry`] this grant
    /// listed needs the grant alive to be read: once it is released or dropped,
    /// reading an entry fails ([`FileDialogError::AccessLost`] when the failure can
    /// be told apart, otherwise [`FileDialogError::Io`]).
    ///
    /// On Android this ends the persisted permission for the folder, which also
    /// ends it for any other grant on the same folder (a restored copy, or one
    /// held elsewhere in the app), so their tokens stop being honoured.
    pub fn release(self) {
        backend::release_folder(self.state, &self.token);
    }
}

/// What [`FolderGrant::list`] returns.
///
/// ```
/// use frust_file_dialogs::ListOptions;
///
/// let options = ListOptions::new().recursive(true).extensions(&["png", "jpg"]);
/// assert_eq!(options.max_depth, 8);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListOptions {
    /// Descends into subfolders. Off by default.
    pub recursive: bool,
    /// Levels of subfolders entered when `recursive`; `0` lists only the
    /// folder itself. Defaults to 8.
    pub max_depth: u32,
    /// The most entries returned; a listing that reaches it is cut short.
    /// Defaults to 10 000.
    pub max_entries: u32,
    /// Keeps only these extensions, normalised as in
    /// [`FileFilter`](crate::FileFilter). Empty keeps every file.
    pub extensions: Vec<String>,
    /// Includes hidden files and descends into hidden folders. Off by default.
    pub include_hidden: bool,
    #[allow(dead_code)] // Read by the walk and the mobile arms.
    pub(crate) cancel: Option<CancelToken>,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_depth: DEFAULT_MAX_DEPTH,
            max_entries: DEFAULT_MAX_ENTRIES,
            extensions: Vec::new(),
            include_hidden: false,
            cancel: None,
        }
    }
}

impl ListOptions {
    /// A flat listing of every non-hidden file, with the default caps.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`recursive`](ListOptions::recursive).
    pub fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Sets [`max_depth`](ListOptions::max_depth).
    pub fn max_depth(mut self, max_depth: u32) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Sets [`max_entries`](ListOptions::max_entries).
    pub fn max_entries(mut self, max_entries: u32) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Replaces [`extensions`](ListOptions::extensions), normalising each one.
    pub fn extensions(mut self, extensions: &[&str]) -> Self {
        self.extensions = normalize_extensions(extensions);
        self
    }

    /// Sets [`include_hidden`](ListOptions::include_hidden).
    pub fn include_hidden(mut self, include_hidden: bool) -> Self {
        self.include_hidden = include_hidden;
        self
    }

    /// Lets the caller cancel the listing through `token`: the walk checks it between entries
    /// and returns [`FileDialogError::Cancelled`]. A token already cancelled fails the call
    /// at once.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// The token set by [`cancel`](Self::cancel), for the platform listings.
    #[allow(dead_code)] // Read by the walk and the mobile arms.
    pub(crate) fn cancel_token(&self) -> Option<&CancelToken> {
        self.cancel.as_ref()
    }

    /// A copy whose `extensions` are normalised even when the field was set directly.
    fn normalized(&self) -> Self {
        let raw: Vec<&str> = self.extensions.iter().map(String::as_str).collect();
        Self {
            extensions: normalize_extensions(&raw),
            ..self.clone()
        }
    }
}

fn normalize_extensions(raw: &[&str]) -> Vec<String> {
    FileFilter::new("", raw).extensions().to_vec()
}

/// One file found by [`FolderGrant::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FolderEntry {
    /// The file name.
    pub name: String,
    /// The path from the listed folder, with `/` as the separator on every platform.
    pub relative_path: String,
    /// The size in bytes, when the platform reports one.
    pub size: Option<u64>,
    /// The last modification time in milliseconds since the Unix epoch.
    pub modified_unix_ms: Option<i64>,
    /// Where the file's bytes are.
    pub location: EntryLocation,
}

/// How a [`FolderEntry`] is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryLocation {
    /// A filesystem path (desktop, iOS).
    Path(PathBuf),
    /// A `content://` URI (Android).
    ContentUri(String),
}

impl FolderEntry {
    /// Copies the file into `dir`, creating `dir` if needed, and returns the new
    /// path. Blocks. An existing file is never replaced: the copy takes a
    /// numbered name instead (`photo-1.png`).
    ///
    /// On Android and iOS this refuses the UI thread with
    /// [`FileDialogError::UiThread`]. On iOS the [`FolderGrant`] that listed the
    /// entry must still be alive; after that the copy fails, with
    /// [`FileDialogError::AccessLost`] when the system denies access.
    pub fn copy_to(&self, dir: &Path) -> Result<PathBuf, FileDialogError> {
        refuse_ui_thread()?;
        if !is_single_component(&self.name) {
            log::warn!("frust-file-dialogs: refusing an entry name that is not a file name");
            return Err(FileDialogError::Io);
        }
        fs::create_dir_all(dir).map_err(io_error)?;
        if let EntryLocation::Path(path) = &self.location {
            probe_access(path)?;
        }
        let destination = reserve_unique_name(dir, &self.name)?;
        let copied = match &self.location {
            EntryLocation::Path(path) => copy_file(path, &destination),
            EntryLocation::ContentUri(uri) => backend::copy_content_uri(uri, &destination),
        };
        match copied {
            Ok(()) => Ok(destination),
            Err(err) => {
                let _ = fs::remove_file(&destination);
                Err(err)
            }
        }
    }

    /// Reads the whole file. Blocks. A file larger than `max_bytes` is refused
    /// with [`FileDialogError::Io`] rather than returned truncated.
    ///
    /// On Android and iOS this refuses the UI thread with
    /// [`FileDialogError::UiThread`]. On iOS the [`FolderGrant`] that listed the
    /// entry must still be alive; after that the read fails, with
    /// [`FileDialogError::AccessLost`] when the system denies access.
    pub fn read_to_vec(&self, max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
        refuse_ui_thread()?;
        match &self.location {
            EntryLocation::Path(path) => read_capped(path, max_bytes),
            EntryLocation::ContentUri(uri) => backend::read_content_uri(uri, max_bytes),
        }
    }
}

/// The segments of a [`FolderGrant::entry`] path, checked before any platform call.
///
/// `Err(Io)` for a path no listing can produce: empty, starting with `/`, holding `\` or
/// NUL, or with a segment the listing's own name rule ([`is_safe_entry_name`]) refuses.
/// `Ok(None)` when every segment is safe but one is hidden (starts with `.`), since a
/// listing skips those. Otherwise the segments, in order.
fn entry_segments(relative_path: &str) -> Result<Option<Vec<&str>>, FileDialogError> {
    let refused = relative_path.is_empty()
        || relative_path.starts_with('/')
        || relative_path.contains(['\\', '\0']);
    let segments: Vec<&str> = relative_path.split('/').collect();
    if refused || !segments.iter().all(|segment| is_safe_entry_name(segment)) {
        log::warn!("frust-file-dialogs: refusing an entry path a listing cannot produce");
        return Err(FileDialogError::Io);
    }
    if segments.iter().any(|segment| segment.starts_with('.')) {
        return Ok(None);
    }
    Ok(Some(segments))
}

/// Whether `name` is one path component by the rule the Android listing applies to a
/// provider's display names (`isSafeEntryName` in the Kotlin host): not empty, `.` or `..`
/// once trimmed, and free of `/`, `\` and NUL.
fn is_safe_entry_name(name: &str) -> bool {
    let trimmed = name.trim();
    !(trimmed.is_empty() || trimmed == "." || trimmed == "..") && !name.contains(['/', '\\', '\0'])
}

/// True for a bare file name; a name from a document provider is untrusted and
/// must not be able to climb out of the destination directory.
fn is_single_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Claims a free name in `dir` by creating it empty, so that two concurrent
/// copies can never pick the same one.
fn reserve_unique_name(dir: &Path, name: &str) -> Result<PathBuf, FileDialogError> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    for attempt in 0..MAX_UNIQUE_NAME_ATTEMPTS {
        let candidate = match (attempt, extension) {
            (0, _) => name.to_owned(),
            (n, Some(extension)) => format!("{stem}-{n}.{extension}"),
            (n, None) => format!("{stem}-{n}"),
        };
        let path = dir.join(candidate);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => return Err(io_error(err)),
        }
    }
    log::warn!("frust-file-dialogs: no free file name left in the destination folder");
    Err(FileDialogError::Io)
}

/// Refuses the UI thread where the platform has one to refuse. Android's content-URI arms
/// refuse it themselves, so only iOS (whose entries are plain paths) needs the check here.
#[cfg(target_os = "ios")]
fn refuse_ui_thread() -> Result<(), FileDialogError> {
    backend::refuse_ui_thread()
}

#[cfg(not(target_os = "ios"))]
#[allow(clippy::unnecessary_wraps)]
fn refuse_ui_thread() -> Result<(), FileDialogError> {
    Ok(())
}

/// On iOS, tells a read denied for lost security scope apart from other failures: the system
/// answers a path outside a live scope with a permission error.
#[cfg(target_os = "ios")]
fn probe_access(path: &Path) -> Result<(), FileDialogError> {
    match File::open(path) {
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            log::warn!("frust-file-dialogs: access to an entry was denied (grant released?)");
            Err(FileDialogError::AccessLost)
        }
        _ => Ok(()),
    }
}

#[cfg(not(target_os = "ios"))]
#[allow(clippy::unnecessary_wraps)]
fn probe_access(_path: &Path) -> Result<(), FileDialogError> {
    Ok(())
}

fn read_capped(path: &Path, max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
    probe_access(path)?;
    let file = File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > max_bytes {
        log::warn!("frust-file-dialogs: file exceeds the {max_bytes} byte read limit");
        return Err(FileDialogError::Io);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    fn entry(name: &str, path: PathBuf) -> FolderEntry {
        FolderEntry {
            name: name.to_owned(),
            relative_path: name.to_owned(),
            size: None,
            modified_unix_ms: None,
            location: EntryLocation::Path(path),
        }
    }

    #[test]
    fn defaults_match_the_documented_caps() {
        let options = ListOptions::default();
        assert_eq!((options.max_depth, options.max_entries), (8, 10_000));
        assert!(!options.recursive && !options.include_hidden && options.extensions.is_empty());
    }

    #[test]
    fn extensions_set_directly_are_normalised_for_the_backend() {
        let mut options = ListOptions::new();
        options.extensions = vec!["*.PNG".to_owned(), ".png".to_owned(), "".to_owned()];
        assert_eq!(options.normalized().extensions, ["png"]);
    }

    #[test]
    fn copy_to_creates_the_directory_and_never_replaces_a_file() {
        let dir = TestDir::new();
        let source = dir.write("photo.png", b"one");
        let out = dir.path().join("nested/out");
        let e = entry("photo.png", source);
        let first = e.copy_to(&out).unwrap();
        let second = e.copy_to(&out).unwrap();
        assert_eq!(first, out.join("photo.png"));
        assert_eq!(second, out.join("photo-1.png"));
        assert_eq!(std::fs::read(second).unwrap(), b"one");
    }

    #[test]
    fn copy_to_numbers_names_without_an_extension_and_dotfiles() {
        let dir = TestDir::new();
        let source = dir.write("source", b"x");
        let out = dir.path().join("out");
        for (name, expected) in [("README", "README-1"), (".env", ".env-1")] {
            let e = entry(name, source.clone());
            e.copy_to(&out).unwrap();
            assert_eq!(e.copy_to(&out).unwrap(), out.join(expected));
        }
    }

    #[test]
    fn copy_to_refuses_a_name_that_climbs_out_of_the_directory() {
        let dir = TestDir::new();
        let source = dir.write("source", b"x");
        let out = dir.path().join("out");
        for name in ["../evil", "a/b", "..", ""] {
            assert_eq!(
                entry(name, source.clone()).copy_to(&out),
                Err(FileDialogError::Io),
                "{name:?} must be refused"
            );
        }
        assert!(!dir.path().join("evil").exists());
    }

    #[test]
    fn a_failed_copy_to_leaves_no_placeholder() {
        let dir = TestDir::new();
        let out = dir.path().join("out");
        let e = entry("missing.png", dir.path().join("missing.png"));
        assert_eq!(e.copy_to(&out), Err(FileDialogError::Io));
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
    }

    #[test]
    fn entry_paths_a_listing_cannot_produce_are_refused() {
        for path in [
            "",
            "/",
            "/a.txt",
            "/sub/a.txt",
            "a\\b.txt",
            "sub\\a.txt",
            "a\0.txt",
            "sub/\0",
            "a//b.txt",
            "sub/",
            "/sub/",
            ".",
            "..",
            "./a.txt",
            "../a.txt",
            "sub/./a.txt",
            "sub/../a.txt",
            "sub/..",
            " ",
            "sub/ /a.txt",
            " .. ",
            "sub/ . /a.txt",
            ".hidden/..",
        ] {
            assert_eq!(
                entry_segments(path),
                Err(FileDialogError::Io),
                "{path:?} must be refused"
            );
        }
    }

    #[test]
    fn entry_paths_with_a_hidden_segment_find_nothing() {
        for path in [".env", ".cache/a.txt", "sub/.hidden.txt", "sub/.deep/a.txt"] {
            assert_eq!(entry_segments(path), Ok(None), "{path:?}");
        }
    }

    #[test]
    fn entry_paths_a_listing_produces_are_split_into_segments() {
        for (path, expected) in [
            ("a.txt", vec!["a.txt"]),
            ("noext", vec!["noext"]),
            ("sub/deep/two.png", vec!["sub", "deep", "two.png"]),
            ("with space.txt", vec!["with space.txt"]),
            ("a..b/c.d.e", vec!["a..b", "c.d.e"]),
            ("ünï/文件.txt", vec!["ünï", "文件.txt"]),
        ] {
            assert_eq!(entry_segments(path), Ok(Some(expected)), "{path:?}");
        }
    }

    #[test]
    fn the_entry_name_rule_matches_the_listing() {
        for name in ["a", "a.txt", ".env", "...", " a "] {
            assert!(is_safe_entry_name(name), "{name:?}");
        }
        for name in ["", " ", ".", "..", " .. ", "a/b", "a\\b", "a\0b"] {
            assert!(!is_safe_entry_name(name), "{name:?}");
        }
    }

    #[test]
    fn read_to_vec_enforces_the_limit_exactly() {
        let dir = TestDir::new();
        let e = entry("a", dir.write("a", b"12345"));
        assert_eq!(e.read_to_vec(5).unwrap(), b"12345");
        assert_eq!(e.read_to_vec(u64::MAX).unwrap(), b"12345");
        assert_eq!(e.read_to_vec(4), Err(FileDialogError::Io));
    }
}
