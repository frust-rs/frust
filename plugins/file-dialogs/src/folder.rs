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

    /// Gives up the access this grant holds. On Android this also revokes the
    /// persisted permission, so the token stops being honoured.
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
    pub fn copy_to(&self, dir: &Path) -> Result<PathBuf, FileDialogError> {
        if !is_single_component(&self.name) {
            log::warn!("frust-file-dialogs: refusing an entry name that is not a file name");
            return Err(FileDialogError::Io);
        }
        fs::create_dir_all(dir).map_err(io_error)?;
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
    pub fn read_to_vec(&self, max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
        match &self.location {
            EntryLocation::Path(path) => read_capped(path, max_bytes),
            EntryLocation::ContentUri(uri) => backend::read_content_uri(uri, max_bytes),
        }
    }
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

fn read_capped(path: &Path, max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
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

/// Standard-alphabet base64 with padding: the text form of an iOS bookmark token.
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub(crate) fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let group = u32::from(chunk[0]) << 16
                | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
                | u32::from(chunk.get(2).copied().unwrap_or(0));
            for index in 0..4 {
                if index <= chunk.len() {
                    let sextet = (group >> (18 - 6 * index)) & 0x3f;
                    out.push(char::from(ALPHABET[sextet as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// `None` for anything [`encode`] cannot have produced.
    pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
        let bytes = text.as_bytes();
        if !bytes.len().is_multiple_of(4) {
            return None;
        }
        let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
        for (position, chunk) in bytes.chunks(4).enumerate() {
            let is_last = position + 1 == bytes.len() / 4;
            let padding = chunk.iter().rev().take_while(|b| **b == b'=').count();
            if padding > 2 || (padding > 0 && !is_last) {
                return None;
            }
            let mut group = 0u32;
            for byte in &chunk[..4 - padding] {
                let value = ALPHABET.iter().position(|a| a == byte)?;
                group = group << 6 | value as u32;
            }
            group <<= 6 * padding as u32;
            if group.trailing_zeros() < (padding * 8) as u32 {
                return None;
            }
            out.extend_from_slice(&group.to_be_bytes()[1..=3 - padding]);
        }
        Some(out)
    }
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
    fn read_to_vec_enforces_the_limit_exactly() {
        let dir = TestDir::new();
        let e = entry("a", dir.write("a", b"12345"));
        assert_eq!(e.read_to_vec(5).unwrap(), b"12345");
        assert_eq!(e.read_to_vec(u64::MAX).unwrap(), b"12345");
        assert_eq!(e.read_to_vec(4), Err(FileDialogError::Io));
    }

    #[test]
    fn base64_matches_known_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64::encode(plain.as_bytes()), encoded);
            assert_eq!(base64::decode(encoded).unwrap(), plain.as_bytes());
        }
    }

    #[test]
    fn base64_round_trips_every_byte_value() {
        let bytes: Vec<u8> = (0..=255).chain((0..=255).rev()).collect();
        for len in 0..bytes.len() {
            let encoded = base64::encode(&bytes[..len]);
            assert_eq!(base64::decode(&encoded).as_deref(), Some(&bytes[..len]));
        }
    }

    #[test]
    fn base64_rejects_malformed_text() {
        for bad in ["Z", "Zg=", "Z===", "Zg==Zg==", "Z!==", "Zm9v=", "Zh=="] {
            assert_eq!(base64::decode(bad), None, "{bad:?}");
        }
    }
}
