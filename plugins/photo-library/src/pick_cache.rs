//! The app-private cache that mobile picks are copied into, plus file-name sanitising.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::PhotoError;
use crate::media_types::extension_for_mime;

const MAX_NAME_BYTES: usize = 128;
const MAX_EXT_BYTES: usize = 16;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Set while a pick is running or the cache is being cleared.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// The crate-wide "one pick at a time" guard, released on drop. Mobile `pick` holds it for
/// the whole call and [`clear`] holds it for the whole clear, so a clear can never delete the
/// directory of a pick in flight and a pick cannot start mid-clear.
#[derive(Debug)]
pub(crate) struct PickGuard(());

impl PickGuard {
    /// [`PhotoError::Busy`] when the guard is already held.
    pub(crate) fn acquire() -> Result<Self, PhotoError> {
        IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| PickGuard(()))
            .map_err(|_| PhotoError::Busy)
    }
}

impl Drop for PickGuard {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// `<cache_dir>/frust-photo-library/picked`.
pub(crate) fn root() -> Result<PathBuf, PhotoError> {
    let base = frust_paths::cache_dir().ok_or(PhotoError::PlatformNotInitialized)?;
    Ok(base.join("frust-photo-library").join("picked"))
}

/// Creates a fresh, unique directory for one pick call under the cache root.
pub(crate) fn new_call_dir() -> Result<PathBuf, PhotoError> {
    new_call_dir_in(&root()?)
}

pub(crate) fn new_call_dir_in(root: &Path) -> Result<PathBuf, PhotoError> {
    fs::create_dir_all(root)?;
    loop {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{nanos:x}-{:x}-{n:x}", std::process::id()));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Removes every cached pick. A missing cache is success; [`PhotoError::Busy`] while a pick
/// runs.
pub(crate) fn clear() -> Result<(), PhotoError> {
    let _guard = PickGuard::acquire()?;
    clear_in(&root()?)
}

pub(crate) fn clear_in(root: &Path) -> Result<(), PhotoError> {
    match fs::remove_dir_all(root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// A safe single path component derived from a platform-reported name.
///
/// Drops separators and control characters, strips leading dots and spaces, caps the result at
/// 128 bytes keeping the extension, and falls back to `item-<n>.<ext>` when nothing is left.
pub(crate) fn sanitize_file_name(raw: &str, n: usize, fallback_ext: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\'))
        .collect();
    let cleaned = cleaned.trim_start_matches(['.', ' ']).trim_end();
    let (stem, ext) = match cleaned.rfind('.') {
        Some(i) if i > 0 && cleaned.len() - i - 1 <= MAX_EXT_BYTES && i + 1 < cleaned.len() => {
            (&cleaned[..i], &cleaned[i..])
        }
        _ => (cleaned, ""),
    };
    if stem.is_empty() {
        let ext = fallback_ext.trim_start_matches('.');
        return if ext.is_empty() {
            format!("item-{n}")
        } else {
            format!("item-{n}.{ext}")
        };
    }
    let budget = MAX_NAME_BYTES.saturating_sub(ext.len());
    let mut end = stem.len().min(budget);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ext}", &stem[..end])
}

/// The final file name of item `n`: the sanitised display name, given the MIME type's
/// extension when it has none, made unique within `used` and never longer than 128 bytes.
#[cfg_attr(target_os = "ios", allow(dead_code))] // Android only.
pub(crate) fn copy_name(
    display: Option<&str>,
    mime: Option<&str>,
    n: usize,
    used: &mut HashSet<String>,
) -> String {
    let ext = mime.and_then(extension_for_mime).unwrap_or("");
    let name = sanitize_file_name(display.unwrap_or(""), n, ext);
    let (stem, dot_ext) = match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && name.len() - i - 1 <= MAX_EXT_BYTES => {
            (&name[..i], name[i..].to_owned())
        }
        _ if !ext.is_empty() => (name.as_str(), format!(".{ext}")),
        _ => (name.as_str(), String::new()),
    };
    let mut candidate = fit_name(stem, "", &dot_ext);
    let mut suffix = 2;
    while !used.insert(candidate.clone()) {
        candidate = fit_name(stem, &format!("-{suffix}"), &dot_ext);
        suffix += 1;
    }
    candidate
}

/// `stem + tail + dot_ext`, truncating `stem` on a char boundary to stay within the cap.
#[cfg_attr(target_os = "ios", allow(dead_code))] // Android only.
fn fit_name(stem: &str, tail: &str, dot_ext: &str) -> String {
    let budget = MAX_NAME_BYTES.saturating_sub(tail.len() + dot_ext.len());
    let mut end = stem.len().min(budget);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{tail}{dot_ext}", &stem[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "frust-photo-library-test-{tag}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn separators_are_removed() {
        assert_eq!(sanitize_file_name("a/b\\c.jpg", 1, "jpg"), "abc.jpg");
        let s = sanitize_file_name("../../etc/passwd", 1, "jpg");
        assert!(!s.contains('/') && !s.starts_with('.'));
    }

    #[test]
    fn dots_and_empty_fall_back() {
        assert_eq!(sanitize_file_name("..", 3, "png"), "item-3.png");
        assert_eq!(sanitize_file_name("", 0, "mp4"), "item-0.mp4");
        assert_eq!(sanitize_file_name("///", 2, ""), "item-2");
        assert_eq!(sanitize_file_name(".hidden.jpg", 1, "jpg"), "hidden.jpg");
    }

    #[test]
    fn control_chars_are_removed() {
        assert_eq!(sanitize_file_name("a\0b\nc\u{7f}.png", 1, "png"), "abc.png");
        assert_eq!(sanitize_file_name("\0\n", 4, "png"), "item-4.png");
    }

    #[test]
    fn long_names_keep_the_extension() {
        let raw = format!("{}.jpeg", "x".repeat(500));
        let s = sanitize_file_name(&raw, 1, "jpg");
        assert_eq!(s.len(), MAX_NAME_BYTES);
        assert!(s.ends_with(".jpeg"));
        let wide = format!("{}.png", "é".repeat(200));
        let s = sanitize_file_name(&wide, 1, "png");
        assert!(s.len() <= MAX_NAME_BYTES && s.ends_with(".png"));
    }

    #[test]
    fn unique_dirs_never_collide() {
        let root = scratch("unique");
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let d = new_call_dir_in(&root).unwrap();
            assert!(d.is_dir());
            assert!(seen.insert(d));
        }
        clear_in(&root).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn clear_on_a_missing_dir_is_ok() {
        assert!(clear_in(&scratch("missing")).is_ok());
    }

    #[test]
    fn clear_removes_contents() {
        let root = scratch("contents");
        let d = new_call_dir_in(&root).unwrap();
        fs::write(d.join("a.jpg"), b"x").unwrap();
        clear_in(&root).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn guard_is_exclusive_and_blocks_clear() {
        // The only test in any test binary that touches the process-wide guard.
        let guard = PickGuard::acquire().unwrap();
        assert_eq!(PickGuard::acquire().err(), Some(PhotoError::Busy));
        assert_eq!(clear().err(), Some(PhotoError::Busy));
        drop(guard);
        let again = PickGuard::acquire().unwrap();
        drop(again);
    }

    fn name(display: Option<&str>, mime: Option<&str>, used: &mut HashSet<String>) -> String {
        copy_name(display, mime, 0, used)
    }

    #[test]
    fn copy_names_get_the_mime_extension_when_missing() {
        let mut used = HashSet::new();
        assert_eq!(name(Some("IMG"), Some("image/png"), &mut used), "IMG.png");
        assert_eq!(name(Some("a.jpg"), Some("image/png"), &mut used), "a.jpg");
        assert_eq!(name(Some("plain"), None, &mut used), "plain");
        assert_eq!(name(None, Some("video/mp4"), &mut used), "item-0.mp4");
    }

    #[test]
    fn repeated_collisions_count_up() {
        let mut used = HashSet::new();
        let names: Vec<_> = (0..3)
            .map(|_| name(Some("a.jpg"), None, &mut used))
            .collect();
        assert_eq!(names, ["a.jpg", "a-2.jpg", "a-3.jpg"]);
        let bare: Vec<_> = (0..2).map(|_| name(Some("b"), None, &mut used)).collect();
        assert_eq!(bare, ["b", "b-2"]);
    }

    #[test]
    fn collisions_on_a_capped_name_stay_within_the_cap() {
        let mut used = HashSet::new();
        let raw = format!("{}.jpeg", "x".repeat(300));
        let first = name(Some(&raw), None, &mut used);
        assert_eq!(first.len(), MAX_NAME_BYTES);
        let mut seen = HashSet::from([first]);
        for _ in 0..12 {
            let next = name(Some(&raw), None, &mut used);
            assert!(next.len() <= MAX_NAME_BYTES && next.ends_with(".jpeg"));
            assert!(seen.insert(next));
        }
        let wide = "é".repeat(100);
        let a = name(Some(&wide), Some("image/png"), &mut used);
        let b = name(Some(&wide), Some("image/png"), &mut used);
        assert!(a.len() <= MAX_NAME_BYTES && b.len() <= MAX_NAME_BYTES);
        assert!(a != b && b.ends_with(".png"));
    }

    #[test]
    fn a_long_trailing_segment_is_not_an_extension() {
        let mut used = HashSet::new();
        let raw = format!("a.{}", "x".repeat(200));
        let first = name(Some(&raw), None, &mut used);
        let second = name(Some(&raw), None, &mut used);
        assert!(first.len() <= MAX_NAME_BYTES && second.len() <= MAX_NAME_BYTES);
        assert!(first != second && second.ends_with("-2"));
    }
}
