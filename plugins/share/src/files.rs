//! File-payload validation shared by every backend: runs on the host before
//! any platform call, so a bad path never reaches a share sheet.

use std::path::{Path, PathBuf};

use crate::ShareError;

/// The most files one share call accepts.
pub(crate) const MAX_FILES: usize = 64;

/// Check `paths` and return their canonical forms.
///
/// Rules: non-empty, at most [`MAX_FILES`], each path absolute, existing, a
/// regular file, and valid UTF-8 once canonicalised (the Apple backends hand
/// the platform a string). No error carries a path.
///
/// # Errors
/// [`ShareError::EmptyPayload`], [`ShareError::TooManyFiles`] or
/// [`ShareError::InvalidFile`].
pub(crate) fn validate(paths: &[&Path]) -> Result<Vec<PathBuf>, ShareError> {
    if paths.is_empty() {
        return Err(ShareError::EmptyPayload);
    }
    if paths.len() > MAX_FILES {
        return Err(ShareError::TooManyFiles);
    }
    paths
        .iter()
        .map(|p| {
            if !p.is_absolute() {
                return Err(ShareError::InvalidFile);
            }
            let canonical = p.canonicalize().map_err(|_| ShareError::InvalidFile)?;
            let is_file = canonical.metadata().map(|m| m.is_file()).unwrap_or(false);
            if !is_file || canonical.to_str().is_none() {
                return Err(ShareError::InvalidFile);
            }
            Ok(canonical)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("frust-share-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_list_is_refused() {
        assert!(matches!(validate(&[]), Err(ShareError::EmptyPayload)));
    }

    #[test]
    fn relative_missing_and_directory_paths_are_refused() {
        let dir = temp_dir("invalid");
        let missing = dir.join("absent.bin");
        for p in [Path::new("relative.bin"), missing.as_path(), dir.as_path()] {
            assert!(matches!(validate(&[p]), Err(ShareError::InvalidFile)));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn more_than_the_cap_is_refused() {
        let dir = temp_dir("cap");
        let file = dir.join("a.bin");
        std::fs::write(&file, b"x").unwrap();
        let many = vec![file.as_path(); MAX_FILES + 1];
        assert!(matches!(validate(&many), Err(ShareError::TooManyFiles)));
        let at_cap = vec![file.as_path(); MAX_FILES];
        assert_eq!(validate(&at_cap).unwrap().len(), MAX_FILES);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn valid_file_is_canonicalised() {
        let dir = temp_dir("ok");
        let file = dir.join("a.bin");
        std::fs::write(&file, b"x").unwrap();
        let out = validate(&[file.as_path()]).unwrap();
        assert_eq!(out, vec![file.canonicalize().unwrap()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
