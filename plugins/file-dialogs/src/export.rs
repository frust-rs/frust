//! Exporting a file: the outcome type and the copy step shared with
//! [`FolderEntry::copy_to`](crate::FolderEntry::copy_to).

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{CancelToken, FileDialogError};

/// What [`FileDialog::export_file`](crate::FileDialog::export_file) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExportOutcome {
    /// The file was written to the place the user chose.
    Saved,
    /// The user dismissed the dialog; nothing was written.
    Cancelled,
}

/// Logs the detail of a failed file operation and reduces it to the payload-free
/// variant. `std::io::Error`'s text carries the OS reason, never a path.
pub(crate) fn io_error(err: std::io::Error) -> FileDialogError {
    log::warn!("frust-file-dialogs: file operation failed: {err}");
    FileDialogError::Io
}

/// Bytes moved between cancel checks and progress reports.
const CHUNK: usize = 64 * 1024;

/// Staging names tried before giving up.
const MAX_STAGING_ATTEMPTS: u32 = 1024;

/// Copies `source` over `destination`. The bytes are staged in a short, uniquely named hidden
/// sibling that is removed again if the copy fails part-way.
pub(crate) fn copy_file(source: &Path, destination: &Path) -> Result<(), FileDialogError> {
    copy_file_with(source, destination, None, &mut |_, _| {})
}

/// Copies `source` over `destination` in chunks, calling `progress(copied, total)` after each
/// and checking `cancel` before each.
///
/// The bytes go to a temporary sibling that replaces `destination` only once complete, so a
/// cancel or failure leaves no partial file and an existing destination keeps its contents.
/// The sibling's name does not depend on the destination's (a long destination name cannot
/// overflow the filename limit) and is unique per copy in progress, so two copies to one
/// destination never share it. On Unix it is created with mode 0600 and takes the source's
/// mode just before the rename; failing to apply that mode fails the copy.
pub(crate) fn copy_file_with(
    source: &Path,
    destination: &Path,
    cancel: Option<&CancelToken>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), FileDialogError> {
    let cancelled = || cancel.is_some_and(CancelToken::is_cancelled);
    if cancelled() {
        return Err(FileDialogError::Cancelled);
    }
    // Writing onto the source's own path would truncate it.
    if is_same_file(source, destination) {
        return Ok(());
    }
    let mut reader = File::open(source).map_err(io_error)?;
    let metadata = reader.metadata().map_err(io_error)?;
    let total = metadata.len();
    let (partial, mut writer) = create_partial(destination)?;
    let result = copy_chunks(&mut reader, &mut writer, total, &cancelled, progress)
        .and_then(|()| writer.sync_all().map_err(io_error))
        .and_then(|()| apply_mode(&writer, &metadata))
        .and_then(|()| {
            drop(writer);
            std::fs::rename(&partial, destination).map_err(io_error)
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// Applies the source's permissions to the staged file. Narrowing or applying a mode that
/// fails is a copy failure: the file would otherwise end up with a mode the source never had.
#[cfg(unix)]
fn apply_mode(writer: &File, metadata: &std::fs::Metadata) -> Result<(), FileDialogError> {
    writer
        .set_permissions(metadata.permissions())
        .map_err(io_error)
}

/// Other platforms: best effort, as `fs::copy` carries the permissions over.
#[cfg(not(unix))]
fn apply_mode(writer: &File, metadata: &std::fs::Metadata) -> Result<(), FileDialogError> {
    let _ = writer.set_permissions(metadata.permissions());
    Ok(())
}

fn copy_chunks(
    reader: &mut File,
    writer: &mut File,
    total: u64,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), FileDialogError> {
    let mut buffer = vec![0u8; CHUNK];
    let mut copied = 0u64;
    loop {
        if cancelled() {
            return Err(FileDialogError::Cancelled);
        }
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read]).map_err(io_error)?;
        copied += read as u64;
        progress(copied.min(total), total);
    }
}

/// Distinguishes the staging files of copies in progress within this process.
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Creates the staging file next to `destination` and returns it with its path. The name is
/// `.frust-<pid>-<counter>.partial`, independent of the destination's name; a name already in
/// use (for example a stale file from an earlier process with the same pid) is skipped.
fn create_partial(destination: &Path) -> Result<(PathBuf, File), FileDialogError> {
    for _ in 0..MAX_STAGING_ATTEMPTS {
        let id = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
        let partial =
            destination.with_file_name(format!(".frust-{}-{id}.partial", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&partial) {
            Ok(file) => return Ok((partial, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => return Err(io_error(err)),
        }
    }
    log::warn!("frust-file-dialogs: no free staging name left in the destination folder");
    Err(FileDialogError::Io)
}

fn is_same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    #[test]
    fn copies_over_an_existing_destination() {
        let dir = TestDir::new();
        let source = dir.write("source.txt", b"new contents");
        let destination = dir.write("destination.txt", b"old");
        copy_file(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new contents");
        assert_eq!(std::fs::read(&source).unwrap(), b"new contents");
    }

    #[test]
    fn copying_a_file_onto_itself_keeps_its_contents() {
        let dir = TestDir::new();
        let source = dir.write("same.txt", b"keep me");
        copy_file(&source, &source).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"keep me");
    }

    #[test]
    fn a_failed_copy_reports_io_and_creates_nothing() {
        let dir = TestDir::new();
        let destination = dir.path().join("destination.txt");
        let result = copy_file(&dir.path().join("missing.txt"), &destination);
        assert_eq!(result, Err(FileDialogError::Io));
        assert!(!destination.exists());
    }

    fn big_source(dir: &TestDir) -> PathBuf {
        dir.write("big.bin", &vec![7u8; CHUNK * 4 + 123])
    }

    fn leftovers(dir: &TestDir) -> Vec<String> {
        std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_cancelled_copy_leaves_no_partial_file() {
        let dir = TestDir::new();
        let source = big_source(&dir);
        let destination = dir.path().join("out.bin");
        let token = CancelToken::new();
        let mut chunks = 0;
        let result = copy_file_with(&source, &destination, Some(&token), &mut |_, _| {
            chunks += 1;
            if chunks == 2 {
                token.cancel();
            }
        });
        assert_eq!(result, Err(FileDialogError::Cancelled));
        assert_eq!(chunks, 2);
        assert!(!destination.exists());
        assert_eq!(leftovers(&dir), ["big.bin"]);
    }

    #[test]
    fn a_cancelled_copy_keeps_an_existing_destination() {
        let dir = TestDir::new();
        let source = big_source(&dir);
        let destination = dir.write("out.bin", b"old contents");
        let token = CancelToken::new();
        let result = copy_file_with(&source, &destination, Some(&token), &mut |_, _| {
            token.cancel();
        });
        assert_eq!(result, Err(FileDialogError::Cancelled));
        assert_eq!(std::fs::read(&destination).unwrap(), b"old contents");
        assert_eq!(leftovers(&dir).len(), 2);
    }

    #[test]
    fn an_already_cancelled_copy_touches_nothing() {
        let dir = TestDir::new();
        let source = big_source(&dir);
        let destination = dir.write("out.bin", b"old");
        let token = CancelToken::new();
        token.cancel();
        let result = copy_file_with(&source, &destination, Some(&token), &mut |_, _| {});
        assert_eq!(result, Err(FileDialogError::Cancelled));
        assert_eq!(std::fs::read(&destination).unwrap(), b"old");
    }

    #[test]
    fn progress_counts_bytes_up_to_the_total() {
        let dir = TestDir::new();
        let source = big_source(&dir);
        let destination = dir.path().join("out.bin");
        let mut seen = Vec::new();
        copy_file_with(&source, &destination, None, &mut |c, t| seen.push((c, t))).unwrap();
        let total = (CHUNK * 4 + 123) as u64;
        assert_eq!(seen.len(), 5);
        assert!(seen.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(seen.last(), Some(&(total, total)));
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert_eq!(leftovers(&dir).len(), 2);
    }

    #[test]
    fn copying_an_empty_file_creates_an_empty_destination() {
        let dir = TestDir::new();
        let source = dir.write("empty", b"");
        let destination = dir.path().join("out");
        copy_file(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"");
    }

    #[test]
    fn a_long_destination_name_does_not_overflow_the_staging_name() {
        let dir = TestDir::new();
        let source = dir.write("source.txt", b"payload");
        let name = format!("{}.txt", "n".repeat(240));
        let destination = dir.path().join(&name);
        copy_file(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"payload");
        assert_eq!(leftovers(&dir).len(), 2);
    }

    #[test]
    fn staging_names_are_short_and_unique_per_copy() {
        let dir = TestDir::new();
        let destination = dir.path().join("out.bin");
        let (first, _a) = create_partial(&destination).unwrap();
        let (second, _b) = create_partial(&destination).unwrap();
        assert_ne!(first, second);
        for partial in [&first, &second] {
            let name = partial.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.len() < 48 && !name.contains("out.bin"), "{name}");
        }
    }

    #[test]
    fn two_copies_to_one_destination_both_complete() {
        let dir = TestDir::new();
        let source = dir.write("source.bin", &vec![3u8; CHUNK * 3]);
        let destination = dir.path().join("out.bin");
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| copy_file(&source, &destination).unwrap());
            }
        });
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert_eq!(leftovers(&dir).len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn the_staged_file_is_private_and_takes_the_source_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let source = dir.write("source.txt", b"data");
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        let destination = dir.path().join("out.txt");
        let (partial, _file) = create_partial(&destination).unwrap();
        let staged = std::fs::metadata(&partial).unwrap().permissions().mode();
        assert_eq!(staged & 0o077, 0, "group and other get nothing");
        std::fs::remove_file(&partial).unwrap();
        copy_file(&source, &destination).unwrap();
        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }
}
