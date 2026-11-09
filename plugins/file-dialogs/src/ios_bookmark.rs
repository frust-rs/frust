//! Persistent folder access on iOS: the security scope a grant holds and the
//! bookmark token that restores it.
//!
//! A folder picked with `asCopy` off arrives as a security-scoped URL. The scope is entered with
//! `startAccessingSecurityScopedResource` and left with exactly one
//! `stopAccessingSecurityScopedResource`, which [`SecurityScope`] pairs through `Drop`.
//! `NSURL.h` warns that a scope never left leaks kernel resources until relaunch.
//!
//! # Tokens
//!
//! A token is the base64 of `bookmarkDataWithOptions:`. `NSURL.h` marks both
//! `...WithSecurityScope` options unavailable on iOS, so a plain bookmark is created, inside the
//! scope, with no options, and resolved with
//! `NSURLBookmarkResolutionWithoutImplicitStartAccessing` so that the one explicit start is the
//! only one to balance.
//!
//! `startAccessingSecurityScopedResource` also answers NO for a URL that needs no scope, such as
//! a folder inside the app's own container. Such a folder is held without a scope when it can be
//! read, and is refused when it cannot.

use std::fs;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{
    NSData, NSError, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};

use crate::folder::base64;
use crate::{FileDialogError, FolderGrant};

/// One entered security scope, left when dropped.
pub(super) struct SecurityScope(Retained<NSURL>);

impl SecurityScope {
    /// Enters `url`'s scope. `None` when it has none to enter, and then nothing is owed.
    pub(super) fn start(url: &Retained<NSURL>) -> Option<Self> {
        // SAFETY: `startAccessingSecurityScopedResource` takes no arguments on a live `NSURL`,
        // which Foundation marks Sendable. A YES is balanced by this value's `Drop`.
        unsafe { url.startAccessingSecurityScopedResource() }.then(|| Self(url.clone()))
    }
}

impl Drop for SecurityScope {
    fn drop(&mut self) {
        // SAFETY: `start` returned YES for this URL and `drop` runs once, so this is the one
        // balancing call `NSURL.h` requires.
        unsafe { self.0.stopAccessingSecurityScopedResource() };
    }
}

/// What a [`FolderGrant`] holds on iOS. Dropping it, by `release` or with the grant, leaves the
/// scope.
pub(crate) struct GrantState {
    _scope: Option<SecurityScope>,
}

fn log_ns_error(what: &str, error: &NSError) {
    // Domain and code only: the localized description can carry a path.
    log::warn!(
        "frust-file-dialogs: {what} failed: {} {}",
        error.domain(),
        error.code()
    );
}

/// The bookmark for `url`, which must be inside its scope when it has one.
fn bookmark(url: &NSURL) -> Result<Vec<u8>, Retained<NSError>> {
    url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
        NSURLBookmarkCreationOptions(0),
        None,
        None,
    )
    .map(|data| data.to_vec())
}

fn is_readable(path: &Path) -> bool {
    fs::read_dir(path).is_ok()
}

/// The grant for a folder the picker just returned.
pub(super) fn grant_from_pick(url: &Retained<NSURL>) -> Result<FolderGrant, FileDialogError> {
    let scope = SecurityScope::start(url);
    let path = url.to_file_path().ok_or(FileDialogError::Platform)?;
    if scope.is_none() && !is_readable(&path) {
        log::warn!("frust-file-dialogs: the picked folder cannot be accessed");
        return Err(FileDialogError::Platform);
    }
    let data = bookmark(url).map_err(|error| {
        log_ns_error("creating a folder bookmark", &error);
        FileDialogError::Platform
    })?;
    Ok(grant(base64::encode(&data), path, scope))
}

/// Re-acquires the folder behind `token`. Any token that no longer leads to a readable folder is
/// [`FileDialogError::AccessLost`].
pub(super) fn restore(token: &str) -> Result<FolderGrant, FileDialogError> {
    let data = base64::decode(token)
        .filter(|data| !data.is_empty())
        .ok_or_else(|| {
            log::warn!("frust-file-dialogs: the folder token is not a bookmark");
            FileDialogError::AccessLost
        })?;
    let mut is_stale = Bool::NO;
    // SAFETY: `is_stale` is a valid `BOOL` that outlives the call; the data and options are
    // live, and a null `relativeToURL:` is allowed by `NSURL.h`.
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &NSData::with_bytes(&data),
            NSURLBookmarkResolutionOptions::WithoutImplicitStartAccessing,
            None,
            &mut is_stale,
        )
    }
    .map_err(|error| {
        log_ns_error("resolving a folder bookmark", &error);
        FileDialogError::AccessLost
    })?;
    let scope = SecurityScope::start(&url);
    let path = url.to_file_path().ok_or(FileDialogError::AccessLost)?;
    if scope.is_none() && !is_readable(&path) {
        log::warn!("frust-file-dialogs: the restored folder cannot be accessed");
        return Err(FileDialogError::AccessLost);
    }
    let token = if is_stale.as_bool() {
        refreshed_token(&url, token)
    } else {
        token.to_owned()
    };
    Ok(grant(token, path, scope))
}

/// A new token for a stale bookmark. The access is held either way, so a failed refresh keeps
/// the old token, which still resolved.
fn refreshed_token(url: &NSURL, old: &str) -> String {
    match bookmark(url) {
        Ok(data) => base64::encode(&data),
        Err(error) => {
            log_ns_error("refreshing a stale folder bookmark", &error);
            old.to_owned()
        }
    }
}

fn grant(token: String, path: PathBuf, scope: Option<SecurityScope>) -> FolderGrant {
    FolderGrant {
        token,
        local_path: Some(path),
        state: GrantState { _scope: scope },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ListOptions;
    use crate::test_dir::TestDir;

    #[test]
    fn a_token_that_is_not_a_bookmark_is_access_lost() {
        for token in ["", "not base64!", "Zm9v", "AAAAAAAAAAAAAAAA"] {
            assert!(
                matches!(restore(token), Err(FileDialogError::AccessLost)),
                "{token:?}"
            );
        }
    }

    #[test]
    fn a_folder_without_a_scope_round_trips_through_its_token() {
        let dir = TestDir::new();
        dir.write("kept.txt", b"x");
        let url = NSURL::from_directory_path(dir.path()).unwrap();
        let picked = grant_from_pick(&url).unwrap();
        assert!(!picked.token().is_empty());
        let token = picked.token().to_owned();
        drop(picked);

        let restored = restore(&token).unwrap();
        let listed = restored.list(&ListOptions::new()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "kept.txt");
        restored.release();
    }

    #[test]
    fn a_folder_that_is_gone_is_refused_at_pick_time() {
        let dir = TestDir::new();
        let gone = NSURL::from_directory_path(dir.path().join("gone")).unwrap();
        assert!(matches!(
            grant_from_pick(&gone),
            Err(FileDialogError::Platform)
        ));
    }

    #[test]
    fn a_token_whose_folder_was_deleted_is_access_lost() {
        let dir = TestDir::new();
        let folder = dir.path().join("folder");
        fs::create_dir_all(&folder).unwrap();
        let url = NSURL::from_directory_path(&folder).unwrap();
        let token = base64::encode(&bookmark(&url).unwrap());
        fs::remove_dir_all(&folder).unwrap();
        assert!(matches!(restore(&token), Err(FileDialogError::AccessLost)));
    }
}
