//! Desktop backend: maps a [`Request`] onto `rfd`'s synchronous dialog.

use std::path::PathBuf;

use crate::{Kind, Request};

/// Shows the dialog and blocks; an empty result means the user cancelled.
pub(crate) fn show(request: Request) -> Vec<PathBuf> {
    let kind = request.kind;
    let dialog = build(request);
    match kind {
        Kind::OpenFile => dialog.pick_file().into_iter().collect(),
        Kind::OpenFiles => dialog.pick_files().unwrap_or_default(),
        Kind::SaveFile => dialog.save_file().into_iter().collect(),
    }
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
