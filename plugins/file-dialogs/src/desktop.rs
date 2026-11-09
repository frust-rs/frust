//! Desktop backend: maps a [`Request`] onto `rfd`'s synchronous dialog; a folder
//! grant is just its absolute path.

use std::path::{Path, PathBuf};

use crate::cancel::ProgressThrottle;
use crate::export::copy_file_with;
use crate::{
    ExportOutcome, FileDialogError, FileProgress, FolderEntry, FolderGrant, Kind, ListOptions,
    Request,
};

/// A desktop grant holds nothing beyond its path.
pub(crate) struct GrantState;

/// Shows the dialog and blocks; an empty result means the user dismissed it.
///
/// A native dialog cannot be closed from another thread, so the token is checked before it
/// shows and again once it returns.
pub(crate) fn show(request: Request) -> Result<Vec<PathBuf>, FileDialogError> {
    request.options.check()?;
    let kind = request.kind;
    let options = request.options.clone();
    let dialog = build(request);
    let picked = match kind {
        Kind::OpenFile => dialog.pick_file().into_iter().collect(),
        Kind::OpenFiles => absolute_only(dialog.pick_files().unwrap_or_default()),
        Kind::SaveFile => dialog.save_file().into_iter().collect(),
        Kind::PickFolder => dialog.pick_folder().into_iter().collect(),
    };
    options.check()?;
    Ok(picked)
}

pub(crate) fn pick_folder(request: Request) -> Result<Option<FolderGrant>, FileDialogError> {
    let mut picked = show(request)?;
    picked.pop().map(grant_for).transpose()
}

pub(crate) fn restore_folder(token: &str) -> Result<FolderGrant, FileDialogError> {
    let path = PathBuf::from(token);
    if !path.is_absolute() || !path.is_dir() {
        return Err(FileDialogError::AccessLost);
    }
    grant_for(path)
}

pub(crate) fn list_folder(
    grant: &FolderGrant,
    options: &ListOptions,
) -> Result<Vec<FolderEntry>, FileDialogError> {
    let root = grant.local_path().ok_or(FileDialogError::AccessLost)?;
    crate::walk::list(root, options)
}

pub(crate) fn release_folder(_state: GrantState, _token: &str) {}

/// Refuses a bad `source` before asking the user anything, then copies it to the
/// chosen destination. The copy runs in chunks, checks the token between them and reports
/// progress.
pub(crate) fn export_file(
    request: Request,
    source: &Path,
) -> Result<ExportOutcome, FileDialogError> {
    if !source.is_file() {
        log::warn!("frust-file-dialogs: the export source is not a readable file");
        return Err(FileDialogError::Io);
    }
    let options = request.options.clone();
    let Some(destination) = show(request)?.pop() else {
        return Ok(ExportOutcome::Cancelled);
    };
    let throttle = options.progress.map(ProgressThrottle::new);
    let report = |done: usize, fraction: f64| {
        if let Some(throttle) = &throttle {
            throttle.report(FileProgress {
                items: 1,
                done,
                fraction,
            });
        }
    };
    report(0, 0.0);
    copy_file_with(
        source,
        &destination,
        options.cancel.as_ref(),
        &mut |copied, total| {
            let fraction = if total == 0 {
                0.0
            } else {
                copied as f64 / total as f64
            };
            report(0, fraction);
        },
    )?;
    report(1, 1.0);
    Ok(ExportOutcome::Saved)
}

pub(crate) fn copy_content_uri(_uri: &str, _destination: &Path) -> Result<(), FileDialogError> {
    Err(FileDialogError::Unsupported)
}

pub(crate) fn read_content_uri(_uri: &str, _max_bytes: u64) -> Result<Vec<u8>, FileDialogError> {
    Err(FileDialogError::Unsupported)
}

/// A token is a `&str`, so a path that is not valid UTF-8 cannot be persisted.
fn grant_for(path: PathBuf) -> Result<FolderGrant, FileDialogError> {
    let Some(token) = path.to_str().map(str::to_owned) else {
        log::warn!("frust-file-dialogs: the chosen folder's path is not valid UTF-8");
        return Err(FileDialogError::Platform);
    };
    Ok(FolderGrant {
        token,
        local_path: Some(path),
        state: GrantState,
    })
}

/// A dialog only ever names absolute paths; the zenity fallback's `|` split can
/// leave relative fragments of one file name, which are never a selection.
fn absolute_only(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().filter(|p| p.is_absolute()).collect()
}

fn build(request: Request) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    if let Some(title) = request.title {
        dialog = dialog.set_title(title);
    }
    if let Some(directory) = request.directory {
        dialog = dialog.set_directory(directory);
    }
    if let Some(name) = request.file_name {
        dialog = dialog.set_file_name(name);
    }
    for filter in &request.filters {
        dialog = dialog.add_filter(filter.name(), filter.extensions());
    }
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileFilter;
    use crate::test_dir::TestDir;

    #[test]
    fn relative_fragments_of_a_split_selection_are_dropped() {
        // `temp_dir` is absolute on every target (`/tmp/..`, `C:\\..`); a bare name never is.
        let report = std::env::temp_dir().join("report");
        let picked = absolute_only(vec![report.clone(), PathBuf::from(".env")]);
        assert_eq!(picked, vec![report]);
    }

    #[test]
    fn build_accepts_a_full_request_without_showing_it() {
        let _ = build(Request {
            kind: Kind::SaveFile,
            title: Some("t".into()),
            directory: Some("/tmp".into()),
            file_name: Some("a.txt".into()),
            filters: vec![FileFilter::new("Text", &["txt"])],
            options: Default::default(),
        });
    }

    #[test]
    fn restore_of_a_missing_folder_is_access_lost() {
        let dir = TestDir::new();
        let token = dir.path().join("gone");
        let result = FolderGrant::restore(token.to_str().unwrap());
        assert_eq!(result.unwrap_err(), FileDialogError::AccessLost);
    }

    #[test]
    fn restore_of_a_file_or_relative_token_is_access_lost() {
        let dir = TestDir::new();
        let file = dir.write("file.txt", b"f");
        for token in [file.to_str().unwrap(), "relative/path", ""] {
            let result = FolderGrant::restore(token);
            assert_eq!(
                result.unwrap_err(),
                FileDialogError::AccessLost,
                "{token:?}"
            );
        }
    }

    #[test]
    fn debug_output_hides_the_token_and_path() {
        let dir = TestDir::new();
        let grant = FolderGrant::restore(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(format!("{grant:?}"), "FolderGrant { .. }");
    }

    #[test]
    fn a_restored_grant_exposes_its_path_lists_and_releases() {
        let dir = TestDir::new();
        dir.write("a.txt", b"a");
        let token = dir.path().to_str().unwrap();
        let grant = FolderGrant::restore(token).unwrap();
        assert_eq!(grant.token(), token);
        assert_eq!(grant.local_path(), Some(dir.path()));
        let entries = grant.list(&ListOptions::new()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.txt");
        grant.release();
    }

    #[test]
    fn a_grant_whose_folder_disappears_reports_access_lost() {
        let dir = TestDir::new();
        let token = dir.write("kept/a.txt", b"a");
        let kept = token.parent().unwrap();
        let grant = FolderGrant::restore(kept.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(kept).unwrap();
        assert_eq!(
            grant.list(&ListOptions::new()),
            Err(FileDialogError::AccessLost)
        );
    }

    #[test]
    fn export_refuses_a_missing_source_before_showing_a_dialog() {
        let dir = TestDir::new();
        let request = Request {
            kind: Kind::SaveFile,
            title: None,
            directory: None,
            file_name: None,
            filters: Vec::new(),
            options: Default::default(),
        };
        assert_eq!(
            export_file(request, &dir.path().join("missing.txt")),
            Err(FileDialogError::Io)
        );
    }

    #[test]
    fn show_refuses_a_cancelled_token_before_any_dialog() {
        let token = crate::CancelToken::new();
        token.cancel();
        let request = Request {
            kind: Kind::OpenFile,
            title: None,
            directory: None,
            file_name: None,
            filters: Vec::new(),
            options: crate::CallOptions {
                progress: None,
                cancel: Some(token),
            },
        };
        assert_eq!(show(request), Err(FileDialogError::Cancelled));
    }
}
