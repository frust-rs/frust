//! Desktop backend: maps a [`Request`] onto `rfd`'s synchronous dialog.

use std::path::PathBuf;

use crate::{FileDialogError, Kind, Request};

/// Shows the dialog and blocks; an empty result means the user cancelled.
pub(crate) fn show(request: Request) -> Result<Vec<PathBuf>, FileDialogError> {
    let kind = request.kind;
    let dialog = build(request);
    Ok(match kind {
        Kind::OpenFile => dialog.pick_file().into_iter().collect(),
        Kind::OpenFiles => absolute_only(dialog.pick_files().unwrap_or_default()),
        Kind::SaveFile => dialog.save_file().into_iter().collect(),
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

    #[test]
    fn relative_fragments_of_a_split_selection_are_dropped() {
        let picked = absolute_only(vec![PathBuf::from("/dir/report"), PathBuf::from(".env")]);
        assert_eq!(picked, vec![PathBuf::from("/dir/report")]);
    }

    #[test]
    fn build_accepts_a_full_request_without_showing_it() {
        let _ = build(Request {
            kind: Kind::SaveFile,
            title: Some("t".into()),
            directory: Some("/tmp".into()),
            file_name: Some("a.txt".into()),
            filters: vec![FileFilter::new("Text", &["txt"])],
        });
    }
}
