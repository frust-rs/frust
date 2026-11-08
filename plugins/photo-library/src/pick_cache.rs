//! The app-private cache that mobile picks are copied into, plus file-name sanitising.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::PhotoError;

const MAX_NAME_BYTES: usize = 128;
const MAX_EXT_BYTES: usize = 16;

static COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// Removes every cached pick. A missing cache is success.
pub(crate) fn clear() -> Result<(), PhotoError> {
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
}
