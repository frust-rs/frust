//! Browsing a user-granted folder as media: [`PhotoLibrary::scan_folder`].

use std::cmp::Reverse;

use frust_file_dialogs::{FolderEntry, FolderGrant, ListOptions};

use crate::media_types;
use crate::{MAX_PAGE_LIMIT, MediaFilter, MediaKind, PhotoError, PhotoLibrary};

const DEFAULT_SCAN_LIMIT: u32 = 100;
/// Entries the listing may walk before it is cut short. The total of a page reflects the cut.
const MAX_SCAN_ENTRIES: u32 = 100_000;

/// What [`PhotoLibrary::scan_folder`] looks for and which page of it to return.
///
/// ```
/// use frust_photo_library::{FolderScan, MediaFilter};
///
/// let scan = FolderScan::new().filter(MediaFilter::Images).recursive(true).limit(50);
/// assert_eq!(scan.limit, 50);
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FolderScan {
    /// Which kinds of media to include.
    pub filter: MediaFilter,
    /// Descends into subfolders. Off by default.
    pub recursive: bool,
    /// Number of matching files to skip, newest first.
    pub offset: u64,
    /// Page size, `1..=500` (a hand-set value is clamped).
    pub limit: u32,
}

impl Default for FolderScan {
    fn default() -> Self {
        Self {
            filter: MediaFilter::default(),
            recursive: false,
            offset: 0,
            limit: DEFAULT_SCAN_LIMIT,
        }
    }
}

impl FolderScan {
    /// Images and videos in the folder itself, the first 100.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`filter`](Self::filter).
    pub fn filter(mut self, filter: MediaFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Sets [`recursive`](Self::recursive).
    pub fn recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self
    }

    /// Sets [`offset`](Self::offset).
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Sets [`limit`](Self::limit), clamped to `1..=500`.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit.clamp(1, MAX_PAGE_LIMIT);
        self
    }

    fn effective_limit(&self) -> usize {
        self.limit.clamp(1, MAX_PAGE_LIMIT) as usize
    }
}

/// A media file found in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderMedia {
    /// The file: read it with [`FolderEntry::read_to_vec`] or copy it with
    /// [`FolderEntry::copy_to`]; `location` says whether it is a path or a content URI.
    pub entry: FolderEntry,
    /// Whether the file is an image or a video, from its extension.
    pub kind: MediaKind,
}

/// One page of a folder scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPage {
    /// The media of this page, newest first.
    pub items: Vec<FolderMedia>,
    /// The number of files matching the scan, before paging.
    pub total: u64,
}

impl PhotoLibrary {
    /// One page of the images and videos in a folder the user granted, newest first. Blocks;
    /// on Android and iOS it refuses the UI thread with [`PhotoError::UiThread`], so run it
    /// with `frust::spawn_blocking`.
    ///
    /// Files are matched by extension and sorted by modification time, newest first, then by
    /// relative path; a file with no known modification time sorts last. Hidden files are
    /// skipped. A listing is capped at 100 000 files, and `total` counts what was listed.
    /// [`PhotoError::PermissionDenied`] means the grant is no longer honoured: ask the user
    /// to pick the folder again. This call needs no cargo feature. It does not exist on
    /// targets without `frust-file-dialogs` (web).
    pub fn scan_folder(grant: &FolderGrant, scan: &FolderScan) -> Result<FolderPage, PhotoError> {
        let options = ListOptions::new()
            .recursive(scan.recursive)
            .max_entries(MAX_SCAN_ENTRIES)
            .extensions(&media_types::extensions_for(scan.filter));
        let entries = grant.list(&options).map_err(map_error)?;
        Ok(page(entries, scan))
    }
}

/// Classifies, sorts and pages `entries`; the part of the scan that needs no platform.
fn page(entries: Vec<FolderEntry>, scan: &FolderScan) -> FolderPage {
    let mut media: Vec<FolderMedia> = entries
        .into_iter()
        .filter_map(|entry| {
            let (kind, _) = media_types::lookup(entry.name.rsplit_once('.')?.1)?;
            let wanted = match scan.filter {
                MediaFilter::Images => kind == MediaKind::Image,
                MediaFilter::Videos => kind == MediaKind::Video,
                _ => true,
            };
            wanted.then_some(FolderMedia { entry, kind })
        })
        .collect();
    // Stable: ties keep the listing's relative-path order. `None` sorts after every time.
    media.sort_by_key(|m| Reverse(m.entry.modified_unix_ms));
    let total = media.len() as u64;
    let skip = usize::try_from(scan.offset).unwrap_or(usize::MAX);
    let items = media
        .into_iter()
        .skip(skip)
        .take(scan.effective_limit())
        .collect();
    FolderPage { items, total }
}

fn map_error(error: frust_file_dialogs::FileDialogError) -> PhotoError {
    use frust_file_dialogs::FileDialogError as E;
    match error {
        E::Unsupported => PhotoError::Unsupported,
        E::UiThread => PhotoError::UiThread,
        E::NoPresenter => PhotoError::NoPresenter,
        E::PlatformNotInitialized => PhotoError::PlatformNotInitialized,
        E::AccessLost => PhotoError::PermissionDenied,
        E::Io => PhotoError::Io(std::io::ErrorKind::Other),
        E::Cancelled => PhotoError::Cancelled,
        other => PhotoError::Platform(other.to_string()),
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "frust-photo-folder-scan-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        /// Writes `relative` with a modification time `age_secs` before a fixed instant.
        fn file(&self, relative: &str, age_secs: u64) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = File::create(&path).unwrap();
            let base = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
            file.set_modified(base - Duration::from_secs(age_secs))
                .unwrap();
        }

        fn grant(&self) -> FolderGrant {
            FolderGrant::restore(self.0.to_str().unwrap()).unwrap()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(page: &FolderPage) -> Vec<&str> {
        page.items
            .iter()
            .map(|m| m.entry.relative_path.as_str())
            .collect()
    }

    #[test]
    fn filters_by_kind() {
        let tree = Tree::new("kind");
        tree.file("a.jpg", 10);
        tree.file("b.MP4", 20);
        tree.file("notes.txt", 5);
        let grant = tree.grant();

        let all = PhotoLibrary::scan_folder(&grant, &FolderScan::new()).unwrap();
        assert_eq!(names(&all), ["a.jpg", "b.MP4"]);
        assert_eq!(all.items[0].kind, MediaKind::Image);
        assert_eq!(all.items[1].kind, MediaKind::Video);

        let images =
            PhotoLibrary::scan_folder(&grant, &FolderScan::new().filter(MediaFilter::Images))
                .unwrap();
        assert_eq!(names(&images), ["a.jpg"]);
        let videos =
            PhotoLibrary::scan_folder(&grant, &FolderScan::new().filter(MediaFilter::Videos))
                .unwrap();
        assert_eq!(names(&videos), ["b.MP4"]);
        assert_eq!(videos.total, 1);
    }

    #[test]
    fn orders_newest_first_with_path_ties() {
        let tree = Tree::new("order");
        tree.file("old.png", 100);
        tree.file("new.png", 1);
        tree.file("b-tie.png", 50);
        tree.file("a-tie.png", 50);
        let page = PhotoLibrary::scan_folder(&tree.grant(), &FolderScan::new()).unwrap();
        assert_eq!(
            names(&page),
            ["new.png", "a-tie.png", "b-tie.png", "old.png"]
        );
    }

    #[test]
    fn recursion_is_opt_in() {
        let tree = Tree::new("recursive");
        tree.file("top.jpg", 1);
        tree.file("sub/deep.jpg", 2);
        let grant = tree.grant();
        let flat = PhotoLibrary::scan_folder(&grant, &FolderScan::new()).unwrap();
        assert_eq!(names(&flat), ["top.jpg"]);
        let deep = PhotoLibrary::scan_folder(&grant, &FolderScan::new().recursive(true)).unwrap();
        assert_eq!(names(&deep), ["top.jpg", "sub/deep.jpg"]);
    }

    #[test]
    fn pages_report_the_unpaged_total() {
        let tree = Tree::new("paging");
        for i in 0..5u64 {
            tree.file(&format!("p{i}.jpg"), i + 1);
        }
        let grant = tree.grant();
        let first = PhotoLibrary::scan_folder(&grant, &FolderScan::new().limit(2)).unwrap();
        assert_eq!((names(&first), first.total), (vec!["p0.jpg", "p1.jpg"], 5));
        let last =
            PhotoLibrary::scan_folder(&grant, &FolderScan::new().limit(2).offset(4)).unwrap();
        assert_eq!((names(&last), last.total), (vec!["p4.jpg"], 5));
        let past = PhotoLibrary::scan_folder(&grant, &FolderScan::new().offset(99)).unwrap();
        assert_eq!((past.items.len(), past.total), (0, 5));
    }

    #[test]
    fn a_hand_set_limit_is_clamped() {
        let mut scan = FolderScan::new();
        scan.limit = 0;
        assert_eq!(scan.effective_limit(), 1);
        scan.limit = 9_999;
        assert_eq!(scan.effective_limit(), 500);
        assert_eq!(FolderScan::new().limit(0).limit, 1);
    }

    #[test]
    fn a_missing_modification_time_sorts_last() {
        let entry = |name: &str, modified: Option<i64>| {
            // `FolderEntry` is non-exhaustive: build one from a real listing and edit it.
            let tree = Tree::new(&format!("none-{name}"));
            tree.file(name, 1);
            let grant = tree.grant();
            let mut e = grant.list(&ListOptions::new()).unwrap().remove(0);
            e.modified_unix_ms = modified;
            e
        };
        let page = page(
            vec![entry("unknown.jpg", None), entry("known.jpg", Some(5))],
            &FolderScan::new(),
        );
        assert_eq!(names(&page), ["known.jpg", "unknown.jpg"]);
    }

    #[test]
    fn a_lost_grant_is_a_permission_error() {
        assert_eq!(
            map_error(frust_file_dialogs::FileDialogError::AccessLost),
            PhotoError::PermissionDenied
        );
        assert!(FolderGrant::restore(Path::new("relative").to_str().unwrap()).is_err());
    }
}
