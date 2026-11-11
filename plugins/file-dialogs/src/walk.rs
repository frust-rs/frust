//! Lists a folder with `std::fs`, and looks up one file in it: the desktop and iOS listing
//! backend.

use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::{CancelToken, EntryLocation, FileDialogError, FolderEntry, ListOptions};

struct PendingDir {
    path: PathBuf,
    relative: String,
    depth: u32,
}

/// Lists the regular files under `root`, sorted by relative path.
///
/// Links are never followed nor listed, so a listing cannot leave the tree. An
/// unreadable `root` is an error; an unreadable subfolder is logged and skipped.
/// `options.extensions` must already be normalised.
pub(crate) fn list(
    root: &Path,
    options: &ListOptions,
) -> Result<Vec<FolderEntry>, FileDialogError> {
    let max_entries = usize::try_from(options.max_entries).unwrap_or(usize::MAX);
    let cancelled = || {
        options
            .cancel_token()
            .is_some_and(CancelToken::is_cancelled)
    };
    if cancelled() {
        return Err(FileDialogError::Cancelled);
    }
    let mut found = Vec::new();
    let mut pending = vec![PendingDir {
        path: root.to_path_buf(),
        relative: String::new(),
        depth: 0,
    }];

    'walk: while let Some(dir) = pending.pop() {
        if cancelled() {
            return Err(FileDialogError::Cancelled);
        }
        let children = match sorted_children(&dir.path) {
            Ok(children) => children,
            Err(err) if dir.depth == 0 => return Err(root_error(err)),
            Err(err) => {
                log::warn!("frust-file-dialogs: skipping an unreadable subfolder: {err}");
                continue;
            }
        };
        let mut subdirs = Vec::new();
        for path in children {
            if cancelled() {
                return Err(FileDialogError::Cancelled);
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            let name = entry_name(&path);
            if is_reparse_point(&metadata)
                || (!options.include_hidden && is_hidden(&name, &metadata))
            {
                continue;
            }
            let relative = if dir.relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", dir.relative)
            };
            if metadata.is_dir() {
                if options.recursive && dir.depth < options.max_depth {
                    subdirs.push(PendingDir {
                        path,
                        relative,
                        depth: dir.depth + 1,
                    });
                }
            } else if metadata.is_file() && matches_extension(&path, &options.extensions) {
                if found.len() >= max_entries {
                    log::warn!("frust-file-dialogs: listing truncated at {max_entries} entries");
                    break 'walk;
                }
                found.push(file_entry(name, relative, path, &metadata));
            }
        }
        pending.extend(subdirs.into_iter().rev());
    }

    found.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(found)
}

/// The regular file at `segments` under `root`, as [`list`] reports it, or `None` when
/// there is none: nothing by that name, a folder, or a link or hidden entry at any segment
/// (the walk neither follows nor lists links, and skips hidden entries by default).
///
/// Each segment is matched against its folder's children byte for byte, so a
/// case-insensitive or normalising filesystem cannot hand back a differently spelled file.
/// An unreadable `root` is an error as in [`list`]; a folder that vanishes part way is
/// `None`. `segments` must already be validated (no empty, `.` or `..` segment).
pub(crate) fn entry(
    root: &Path,
    segments: &[&str],
) -> Result<Option<FolderEntry>, FileDialogError> {
    let mut dir = root.to_path_buf();
    for (index, segment) in segments.iter().enumerate() {
        let found = match child_named(&dir, segment) {
            Ok(found) => found,
            Err(err) if index == 0 => return Err(root_error(err)),
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(None);
            }
            Err(err) => return Err(crate::export::io_error(err)),
        };
        let Some(path) = found else {
            return Ok(None);
        };
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            return Ok(None);
        };
        let name = entry_name(&path);
        if is_reparse_point(&metadata) || is_hidden(&name, &metadata) {
            return Ok(None);
        }
        if index + 1 < segments.len() {
            if !metadata.is_dir() {
                return Ok(None);
            }
            dir = path;
        } else if metadata.is_file() {
            return Ok(Some(file_entry(name, segments.join("/"), path, &metadata)));
        }
    }
    Ok(None)
}

/// The child of `dir` named exactly `name`, or `None`.
fn child_named(dir: &Path, name: &str) -> io::Result<Option<PathBuf>> {
    for child in fs::read_dir(dir)? {
        let child = child?;
        if child.file_name() == OsStr::new(name) {
            return Ok(Some(child.path()));
        }
    }
    Ok(None)
}

/// The entry for the regular file at `path`, shared by [`list`] and [`entry`] so that a
/// looked-up entry equals the listed one field for field.
fn file_entry(name: String, relative: String, path: PathBuf, metadata: &Metadata) -> FolderEntry {
    FolderEntry {
        name,
        relative_path: relative,
        size: Some(metadata.len()),
        modified_unix_ms: modified_unix_ms(metadata),
        location: EntryLocation::Path(path),
    }
}

/// One directory's children in name order, so that which entries survive the
/// entry cap does not depend on the order the OS returns them in.
fn sorted_children(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut children = fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<Vec<_>>>()?;
    children.sort();
    Ok(children)
}

fn root_error(err: io::Error) -> FileDialogError {
    match err.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::NotADirectory => {
            log::warn!("frust-file-dialogs: folder no longer accessible: {err}");
            FileDialogError::AccessLost
        }
        _ => crate::export::io_error(err),
    }
}

fn entry_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn matches_extension(path: &Path, extensions: &[String]) -> bool {
    extensions.is_empty()
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| extensions.contains(&ext.to_lowercase()))
}

fn modified_unix_ms(metadata: &Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).ok(),
        Err(before) => i64::try_from(before.duration().as_millis())
            .ok()
            .map(|ms| -ms),
    }
}

#[cfg(windows)]
fn is_hidden(name: &str, metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    name.starts_with('.') || metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _metadata: &Metadata) -> bool {
    name.starts_with('.')
}

/// A junction is not a symlink to `symlink_metadata`, which reports it as a plain
/// directory, so it is refused by its reparse attribute instead.
#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    fn relative_paths(entries: &[FolderEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.relative_path.as_str()).collect()
    }

    fn tree() -> TestDir {
        let dir = TestDir::new();
        dir.write("b.txt", b"bb");
        dir.write("a.PNG", b"a");
        dir.write("noext", b"");
        dir.write(".hidden.txt", b"h");
        dir.write(".cache/inside.txt", b"h");
        dir.write("sub/one.txt", b"1");
        dir.write("sub/deep/two.png", b"22");
        dir.write("sub/deep/deeper/three.txt", b"333");
        dir
    }

    #[test]
    fn default_listing_is_flat_and_skips_hidden_entries() {
        let dir = tree();
        let entries = list(dir.path(), &ListOptions::new()).unwrap();
        assert_eq!(relative_paths(&entries), ["a.PNG", "b.txt", "noext"]);
    }

    #[test]
    fn entries_carry_name_size_time_and_location() {
        let dir = tree();
        let entries = list(dir.path(), &ListOptions::new().recursive(true)).unwrap();
        let two = entries
            .iter()
            .find(|e| e.name == "two.png")
            .expect("nested file listed");
        assert_eq!(two.relative_path, "sub/deep/two.png");
        assert_eq!(two.size, Some(2));
        assert!(two.modified_unix_ms.is_some_and(|ms| ms > 0));
        assert_eq!(
            two.location,
            EntryLocation::Path(dir.path().join("sub").join("deep").join("two.png"))
        );
    }

    #[test]
    fn recursive_listing_is_sorted_by_relative_path() {
        let dir = TestDir::new();
        dir.write("a/z.txt", b"");
        dir.write("a.txt", b"");
        dir.write("b/c.txt", b"");
        let entries = list(dir.path(), &ListOptions::new().recursive(true)).unwrap();
        assert_eq!(relative_paths(&entries), ["a.txt", "a/z.txt", "b/c.txt"]);
    }

    #[test]
    fn max_depth_limits_the_levels_entered() {
        let dir = tree();
        let at = |depth| {
            let options = ListOptions::new().recursive(true).max_depth(depth);
            list(dir.path(), &options).unwrap().len()
        };
        assert_eq!(at(0), 3);
        assert_eq!(at(1), 4);
        assert_eq!(at(2), 5);
        assert_eq!(at(3), 6);
        assert_eq!(at(8), 6);
    }

    #[test]
    fn max_entries_caps_the_result_deterministically() {
        let dir = tree();
        let options = ListOptions::new().recursive(true).max_entries(2);
        let first = list(dir.path(), &options).unwrap();
        assert_eq!(relative_paths(&first), ["a.PNG", "b.txt"]);
        assert!(
            list(dir.path(), &options.max_entries(0))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn extension_filter_is_case_insensitive_and_drops_extensionless_files() {
        let dir = tree();
        let options = ListOptions::new().recursive(true).extensions(&["*.png"]);
        let entries = list(dir.path(), &options).unwrap();
        assert_eq!(relative_paths(&entries), ["a.PNG", "sub/deep/two.png"]);
    }

    #[test]
    fn include_hidden_lists_hidden_files_and_folders() {
        let dir = tree();
        let options = ListOptions::new().recursive(true).include_hidden(true);
        let listed = list(dir.path(), &options).unwrap();
        let paths = relative_paths(&listed);
        assert!(paths.contains(&".hidden.txt"));
        assert!(paths.contains(&".cache/inside.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn links_are_neither_followed_nor_listed() {
        use std::os::unix::fs::symlink;
        let outside = TestDir::new();
        let outside_file = outside.write("secret.txt", b"s");
        let dir = TestDir::new();
        dir.write("real.txt", b"r");
        symlink(outside.path(), dir.path().join("linked-dir")).unwrap();
        symlink(&outside_file, dir.path().join("linked-file.txt")).unwrap();
        let entries = list(dir.path(), &ListOptions::new().recursive(true)).unwrap();
        assert_eq!(relative_paths(&entries), ["real.txt"]);
    }

    #[test]
    fn an_already_cancelled_walk_returns_cancelled() {
        let dir = tree();
        let token = CancelToken::new();
        token.cancel();
        let options = ListOptions::new().recursive(true).cancel(token);
        assert_eq!(list(dir.path(), &options), Err(FileDialogError::Cancelled));
    }

    #[test]
    fn a_token_lists_normally_until_it_fires() {
        let dir = tree();
        let token = CancelToken::new();
        let options = ListOptions::new().recursive(true).cancel(token.clone());
        assert_eq!(list(dir.path(), &options).unwrap().len(), 6);
        token.cancel();
        assert_eq!(list(dir.path(), &options), Err(FileDialogError::Cancelled));
    }

    #[test]
    fn a_missing_root_is_access_lost() {
        let dir = TestDir::new();
        let result = list(&dir.path().join("gone"), &ListOptions::new());
        assert_eq!(result, Err(FileDialogError::AccessLost));
    }

    #[test]
    fn a_file_as_root_is_access_lost() {
        let dir = TestDir::new();
        let file = dir.write("file.txt", b"f");
        assert_eq!(
            list(&file, &ListOptions::new()),
            Err(FileDialogError::AccessLost)
        );
    }
}
