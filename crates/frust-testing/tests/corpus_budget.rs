//! Corpus size guard for `testing/goldens/`.
//!
//! Plain git tracks this corpus — no Git LFS — so its size is bounded the
//! same way upstream keeps its own comparable PNG corpus small (upstream's
//! is ~1.43 MB across 512 PNGs; see `testing/goldens/README.md` for the
//! policy this test enforces): fails the build if the corpus exceeds 8 MB
//! total, or any single PNG exceeds 64 KB. A promoted baseline that grows
//! past either budget belongs re-encoded/cropped, not exempted.

use std::fs;
use std::path::{Path, PathBuf};

/// Total corpus budget, bytes.
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
/// Per-PNG budget, bytes.
const MAX_PNG_BYTES: u64 = 64 * 1024;

/// `testing/goldens/` at the repository root, resolved from this crate's own
/// manifest directory so the walk is working-directory independent.
fn goldens_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("testing")
        .join("goldens")
}

/// Every file under `dir`, recursively, as `(path, size_in_bytes)`.
fn walk_files(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_files(&path, out);
        } else if file_type.is_file() {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((path, size));
        }
    }
}

#[test]
fn corpus_stays_within_the_total_and_per_png_budget() {
    let root = goldens_root();
    let mut files = Vec::new();
    walk_files(&root, &mut files);

    let mut total: u64 = 0;
    let mut oversized_pngs = Vec::new();

    for (path, size) in &files {
        total += size;
        let is_png = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"));
        if is_png && *size > MAX_PNG_BYTES {
            oversized_pngs.push(format!(
                "{} ({size} bytes > {MAX_PNG_BYTES} byte cap)",
                path.strip_prefix(&root).unwrap_or(path).display()
            ));
        }
    }

    assert!(
        oversized_pngs.is_empty(),
        "golden PNG(s) exceed the {MAX_PNG_BYTES}-byte per-file budget:\n{}",
        oversized_pngs.join("\n")
    );

    assert!(
        total <= MAX_TOTAL_BYTES,
        "testing/goldens/ is {total} bytes, over the {MAX_TOTAL_BYTES}-byte corpus budget \
         (see testing/goldens/README.md)"
    );
}
