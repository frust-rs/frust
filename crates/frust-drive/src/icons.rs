//! Tool-side icon generation: one 1024² source PNG in, per-platform icon
//! containers out — macOS `.icns` ([`generate_icns`]), Windows `.ico`
//! ([`generate_ico`]), and a Linux `hicolor` icon-theme PNG size set
//! ([`generate_hicolor_set`]).
//!
//! Pure file work, like [`crate::scaffold`] — no [`crate::process::ProcessRunner`]
//! involved, since every step (decode, Lanczos3 downscale, container encode)
//! is in-process via the `image`/`icns` crates. Print-free per this crate's
//! contract (`docs/CODE_STANDARDS.md`'s printing anti-pattern,
//! `tests/print_free_cores.rs`): a source-quality concern comes back as a
//! typed [`SourceWarning`] on the returned [`IconReport`] rather than a
//! `println!`, leaving it to the caller (CLI/TUI) to decide whether and how
//! to show it.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::{GenericImageView, ImageFormat, RgbaImage, imageops::FilterType};

/// Minimum accepted source dimension (px) — below this every generated size
/// would be a meaningful upscale, not just a soft one.
const MIN_SOURCE_SIZE: u32 = 512;

/// Recommended source dimension (px) — the largest size any of the three
/// pipelines below ever emits (the ICNS `512x512@2x` entry), so a source at
/// or above this is never upscaled.
const RECOMMENDED_SOURCE_SIZE: u32 = 1024;

/// Standard macOS iconset sizes as `(logical_size, density)` pairs, resolving
/// to the ten pixel sizes `iconutil`/Xcode's asset catalog expect: 16, 32,
/// 32, 64, 128, 256, 256, 512, 512, 1024. This is the same table
/// `cargo-bundle`'s ICNS packer (`osx_bundle.rs`'s `create_icns_from_svg`)
/// uses, keyed through `icns::IconType::from_pixel_size_and_density` so a
/// density-2 entry (e.g. `16x16@2x`, 32 physical px) resolves to a distinct
/// `IconType` from the density-1 entry at the same pixel size (`32x32@1x`)
/// rather than colliding.
const ICNS_SIZES: [(u32, u32); 10] = [
    (16, 1),
    (16, 2),
    (32, 1),
    (32, 2),
    (128, 1),
    (128, 2),
    (256, 1),
    (256, 2),
    (512, 1),
    (512, 2),
];

/// Windows `.ico` sizes: the classic Explorer/taskbar/shortcut set (16 for a
/// list-view row, 24 for a small toolbar icon, 32 for the desktop/taskbar,
/// 48 for a large list view, 64 for a jumbo view, 256 for the modern
/// PNG-compressed "extra large" entry every icon-authoring guide since
/// Windows Vista recommends).
const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// The Linux `hicolor` icon-theme PNG size set
/// (<https://specifications.freedesktop.org/icon-theme-spec/>).
const HICOLOR_SIZES: [u32; 7] = [16, 32, 48, 64, 128, 256, 512];

/// A non-fatal quality observation about the source PNG. Never fatal by
/// itself — generation still proceeds — but worth surfacing to a human
/// deciding whether to re-export a bigger source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceWarning {
    /// The source is smaller than [`RECOMMENDED_SOURCE_SIZE`]: every
    /// generated size larger than the source's own is upscaled from it via
    /// Lanczos3 and may look soft. `size` is the source's (square) side
    /// length; `recommended` is the size that would avoid any upscale.
    BelowRecommendedSize { size: u32, recommended: u32 },
}

/// The result of one icon-generation call: the container file(s) written,
/// plus any [`SourceWarning`] about the source image that was decoded to
/// produce them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconReport {
    /// The file(s) written — one entry for [`generate_icns`]/[`generate_ico`],
    /// one per [`HICOLOR_SIZES`] entry for [`generate_hicolor_set`].
    pub paths: Vec<PathBuf>,
    pub warning: Option<SourceWarning>,
}

/// Errors this module's three generators can return — a library-contract
/// enum callers match on, per `docs/CODE_STANDARDS.md`'s Error Handling
/// convention.
#[derive(Debug, thiserror::Error)]
pub enum IconError {
    #[error("opening source image '{path}': {source}")]
    OpenSource {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("source image '{path}' is not a PNG file (detected format: {detected:?})")]
    NotPng {
        path: PathBuf,
        detected: Option<ImageFormat>,
    },
    #[error("decoding source image '{path}': {source}")]
    DecodeSource {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("source image '{path}' is {width}x{height}, must be square")]
    NotSquare {
        path: PathBuf,
        width: u32,
        height: u32,
    },
    #[error("source image '{path}' is {size}x{size}, below the {min}x{min} minimum")]
    TooSmall { path: PathBuf, size: u32, min: u32 },
    #[error("no ICNS icon type for a {size}x{size} icon")]
    UnsupportedIcnsSize { size: u32 },
    #[error("building ICNS icon family: {source}")]
    Icns {
        #[source]
        source: std::io::Error,
    },
    #[error("encoding ICO frame ({size}x{size}): {source}")]
    IcoFrame {
        size: u32,
        #[source]
        source: image::ImageError,
    },
    #[error("encoding ICO container: {source}")]
    IcoEncode {
        #[source]
        source: image::ImageError,
    },
    #[error("encoding PNG '{path}': {source}")]
    EncodePng {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
    #[error("creating directory '{path}': {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing '{path}': {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Opens, content-sniffs, decodes, and validates `path`: must be a real PNG
/// (checked by magic bytes via [`image::ImageReader::with_guessed_format`],
/// not just the file extension), square, and at least
/// [`MIN_SOURCE_SIZE`] on a side. Returns the decoded RGBA8 buffer plus an
/// optional [`SourceWarning`] if the source is smaller than
/// [`RECOMMENDED_SOURCE_SIZE`].
fn load_source(path: &Path) -> Result<(RgbaImage, Option<SourceWarning>), IconError> {
    let mut reader = image::ImageReader::open(path).map_err(|source| IconError::OpenSource {
        path: path.to_path_buf(),
        source,
    })?;
    // Clear the extension-derived format guess first: `with_guessed_format`
    // only *replaces* the current format on a successful content sniff and
    // otherwise leaves it untouched, so without this a `.png`-named file
    // with garbage content would silently keep the extension's guess
    // instead of surfacing as content-invalid below.
    reader.clear_format();
    let reader = reader
        .with_guessed_format()
        .map_err(|source| IconError::OpenSource {
            path: path.to_path_buf(),
            source,
        })?;

    let detected = reader.format();
    if detected != Some(ImageFormat::Png) {
        return Err(IconError::NotPng {
            path: path.to_path_buf(),
            detected,
        });
    }

    let decoded = reader.decode().map_err(|source| IconError::DecodeSource {
        path: path.to_path_buf(),
        source,
    })?;

    let (width, height) = decoded.dimensions();
    if width != height {
        return Err(IconError::NotSquare {
            path: path.to_path_buf(),
            width,
            height,
        });
    }
    if width < MIN_SOURCE_SIZE {
        return Err(IconError::TooSmall {
            path: path.to_path_buf(),
            size: width,
            min: MIN_SOURCE_SIZE,
        });
    }

    let warning =
        (width < RECOMMENDED_SOURCE_SIZE).then_some(SourceWarning::BelowRecommendedSize {
            size: width,
            recommended: RECOMMENDED_SOURCE_SIZE,
        });

    Ok((decoded.to_rgba8(), warning))
}

/// Resizes `src` to `size`x`size` via Lanczos3, or returns a cheap clone if
/// it is already that size (never upscale-then-downscale a same-size source).
fn resize_square(src: &RgbaImage, size: u32) -> RgbaImage {
    if src.width() == size && src.height() == size {
        src.clone()
    } else {
        image::imageops::resize(src, size, size, FilterType::Lanczos3)
    }
}

/// Creates `path`'s parent directory tree if it doesn't already exist.
fn ensure_parent_dir(path: &Path) -> Result<(), IconError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| IconError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Opens `path` for writing (creating its parent directories first).
fn create_writer(path: &Path) -> Result<BufWriter<fs::File>, IconError> {
    ensure_parent_dir(path)?;
    let file = fs::File::create(path).map_err(|source| IconError::WriteFile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(BufWriter::new(file))
}

/// Generates a macOS `.icns` icon family at `out` from the square PNG at
/// `src_png`, covering the standard ten-entry iconset table
/// ([`ICNS_SIZES`]). Each entry is resized independently via Lanczos3 (never
/// resized from a previously-resized entry), then packed as RGBA data — the
/// `icns` crate always PNG-encodes on write regardless of pixel format, per
/// its own documented behavior.
pub fn generate_icns(src_png: &Path, out: &Path) -> Result<IconReport, IconError> {
    let (src, warning) = load_source(src_png)?;

    let mut family = icns::IconFamily::new();
    for &(logical, density) in &ICNS_SIZES {
        let pixel_size = logical * density;
        let resized = resize_square(&src, pixel_size);
        let icon_type =
            icns::IconType::from_pixel_size_and_density(pixel_size, pixel_size, density)
                .ok_or(IconError::UnsupportedIcnsSize { size: pixel_size })?;
        let image = icns::Image::from_data(
            icns::PixelFormat::RGBA,
            pixel_size,
            pixel_size,
            resized.into_raw(),
        )
        .map_err(|source| IconError::Icns { source })?;
        family
            .add_icon_with_type(&image, icon_type)
            .map_err(|source| IconError::Icns { source })?;
    }

    let writer = create_writer(out)?;
    family
        .write(writer)
        .map_err(|source| IconError::Icns { source })?;

    Ok(IconReport {
        paths: vec![out.to_path_buf()],
        warning,
    })
}

/// Generates a Windows `.ico` at `out` from the square PNG at `src_png`,
/// covering the classic six-size set ([`ICO_SIZES`]). Each entry is
/// PNG-compressed via `image`'s built-in `ico` feature
/// (`image::codecs::ico::{IcoEncoder, IcoFrame}`) rather than a second,
/// standalone `ico` crate — every ICO reader since Windows Vista accepts a
/// PNG-compressed entry.
pub fn generate_ico(src_png: &Path, out: &Path) -> Result<IconReport, IconError> {
    let (src, warning) = load_source(src_png)?;

    let mut frames = Vec::with_capacity(ICO_SIZES.len());
    for &size in &ICO_SIZES {
        let resized = resize_square(&src, size);
        let frame = image::codecs::ico::IcoFrame::as_png(
            resized.as_raw(),
            size,
            size,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|source| IconError::IcoFrame { size, source })?;
        frames.push(frame);
    }

    let writer = create_writer(out)?;
    image::codecs::ico::IcoEncoder::new(writer)
        .encode_images(&frames)
        .map_err(|source| IconError::IcoEncode { source })?;

    Ok(IconReport {
        paths: vec![out.to_path_buf()],
        warning,
    })
}

/// Generates the Linux `hicolor` icon-theme PNG size set ([`HICOLOR_SIZES`])
/// under `out_dir`, following the theme spec's `<size>x<size>/apps/`
/// layout — `out_dir` is the theme root a caller has already scoped to the
/// target app (e.g. a packaging resources directory); this function names
/// each leaf file `icon.png` since it takes no app-name parameter.
pub fn generate_hicolor_set(src_png: &Path, out_dir: &Path) -> Result<IconReport, IconError> {
    let (src, warning) = load_source(src_png)?;

    let mut paths = Vec::with_capacity(HICOLOR_SIZES.len());
    for &size in &HICOLOR_SIZES {
        let resized = resize_square(&src, size);
        let dest = out_dir
            .join(format!("{size}x{size}"))
            .join("apps")
            .join("icon.png");
        ensure_parent_dir(&dest)?;
        resized
            .save_with_format(&dest, ImageFormat::Png)
            .map_err(|source| IconError::EncodePng {
                path: dest.clone(),
                source,
            })?;
        paths.push(dest);
    }

    Ok(IconReport { paths, warning })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-icons-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a plain-color `size`x`size` PNG to `dir/name.png` and returns
    /// its path — no binary fixtures committed, generated in-test via
    /// `image` itself.
    fn write_test_png(dir: &Path, name: &str, size: u32) -> PathBuf {
        let img = RgbaImage::from_fn(size, size, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        let path = dir.join(format!("{name}.png"));
        img.save_with_format(&path, ImageFormat::Png).unwrap();
        path
    }

    fn write_non_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"not a png file at all").unwrap();
        path
    }

    #[test]
    fn generates_icns_with_all_ten_entries_and_no_warning_at_1024() {
        let dir = unique_temp_dir("icns-1024");
        let src = write_test_png(&dir, "src", 1024);
        let out = dir.join("App.icns");

        let report = generate_icns(&src, &out).expect("icns generation should succeed");
        assert_eq!(report.paths, vec![out.clone()]);
        assert_eq!(report.warning, None);

        let file = std::io::BufReader::new(fs::File::open(&out).unwrap());
        let family = icns::IconFamily::read(file).expect("written .icns should parse back");
        let available = family.available_icons();
        assert_eq!(
            available.len(),
            ICNS_SIZES.len(),
            "expected one entry per ICNS_SIZES row, got {available:?}"
        );
        for &(logical, density) in &ICNS_SIZES {
            let pixel_size = logical * density;
            let icon_type =
                icns::IconType::from_pixel_size_and_density(pixel_size, pixel_size, density)
                    .unwrap();
            assert!(
                available.contains(&icon_type),
                "missing {pixel_size}x{pixel_size} (density {density}) in {available:?}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn icns_warns_when_source_below_recommended_size() {
        let dir = unique_temp_dir("icns-512");
        let src = write_test_png(&dir, "src", 512);
        let out = dir.join("App.icns");

        let report = generate_icns(&src, &out).expect("icns generation should succeed at 512");
        assert_eq!(
            report.warning,
            Some(SourceWarning::BelowRecommendedSize {
                size: 512,
                recommended: RECOMMENDED_SOURCE_SIZE,
            })
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generates_ico_with_all_six_sizes() {
        let dir = unique_temp_dir("ico");
        let src = write_test_png(&dir, "src", 1024);
        let out = dir.join("app.ico");

        let report = generate_ico(&src, &out).expect("ico generation should succeed");
        assert_eq!(report.paths, vec![out.clone()]);

        // Parses back via image's own ICO decoder.
        let decoded = image::ImageReader::open(&out)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .decode()
            .expect("written .ico should parse back");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        // The ICONDIR header (offset 4: u16 LE entry count) plus each
        // 16-byte ICONDIRENTRY's width/height bytes (offsets 6/7 within the
        // entry) give the full size set without relying on the decoder,
        // which only ever surfaces one frame.
        let bytes = fs::read(&out).unwrap();
        let entry_count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        assert_eq!(entry_count, ICO_SIZES.len());
        let mut sizes: Vec<u32> = Vec::with_capacity(entry_count);
        for i in 0..entry_count {
            let entry = &bytes[6 + i * 16..6 + i * 16 + 16];
            let width = if entry[0] == 0 { 256 } else { entry[0] as u32 };
            sizes.push(width);
        }
        sizes.sort_unstable();
        let mut expected = ICO_SIZES.to_vec();
        expected.sort_unstable();
        assert_eq!(sizes, expected);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generates_hicolor_set_with_all_seven_sizes() {
        let dir = unique_temp_dir("hicolor");
        let src = write_test_png(&dir, "src", 1024);
        let out_dir = dir.join("hicolor");

        let report =
            generate_hicolor_set(&src, &out_dir).expect("hicolor generation should succeed");
        assert_eq!(report.paths.len(), HICOLOR_SIZES.len());

        for (&size, path) in HICOLOR_SIZES.iter().zip(report.paths.iter()) {
            assert!(path.exists(), "{path:?} should exist");
            let decoded = image::open(path).expect("each hicolor PNG should parse back");
            assert_eq!(decoded.width(), size);
            assert_eq!(decoded.height(), size);
            assert!(
                path.ends_with(format!("{size}x{size}/apps/icon.png")),
                "{path:?} should follow the hicolor <size>x<size>/apps/ layout"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_square_source() {
        let dir = unique_temp_dir("non-square");
        let img = RgbaImage::from_pixel(1024, 512, image::Rgba([10, 20, 30, 255]));
        let src = dir.join("src.png");
        img.save_with_format(&src, ImageFormat::Png).unwrap();

        let err = generate_icns(&src, &dir.join("out.icns")).unwrap_err();
        assert!(matches!(
            err,
            IconError::NotSquare {
                width: 1024,
                height: 512,
                ..
            }
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_source_below_minimum_size() {
        let dir = unique_temp_dir("too-small");
        let src = write_test_png(&dir, "src", 256);

        let err = generate_ico(&src, &dir.join("out.ico")).unwrap_err();
        assert!(matches!(
            err,
            IconError::TooSmall {
                size: 256,
                min: MIN_SOURCE_SIZE,
                ..
            }
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_png_source() {
        let dir = unique_temp_dir("non-png");
        let src = write_non_png(&dir, "src.png");

        let err = generate_hicolor_set(&src, &dir.join("hicolor")).unwrap_err();
        assert!(matches!(err, IconError::NotPng { .. }));

        let _ = fs::remove_dir_all(&dir);
    }
}
