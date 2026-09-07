//! Deterministic, GPU-free PNG snapshots of the `frust-gallery` case registry
//! — the library half of the `widget-snapshots` binary
//! (`src/bin/widget-snapshots.rs`), whose output the website serves under
//! `static/preview/`.
//!
//! # What this is, and what it is NOT
//!
//! This is a *documentation asset* generator, not a golden gate. [`crate::frame`]
//! plus [`crate::golden`] own the pixel-regression contract (`docs/TESTING.md`'s
//! Golden Image Policy): a committed baseline, a tolerance, an eroded-interior
//! mask, and a hard refusal to promote a frame that shaped against a host font.
//! Nothing here compares against a committed baseline and nothing here is
//! promoted — the images are rebuilt from the registry on demand and rsynced to
//! the website. The one determinism claim this module *does* make is the one
//! the binary's `--check` mode verifies: rendering the same registry twice on
//! the same host produces byte-identical PNGs.
//!
//! # The pipeline
//!
//! One case, one [`frust_gallery::Variant`], one PNG:
//!
//! ```text
//! record_scene   -> frame::record_view(scene, FrameSpec, theme(design, variant), fonts, case.build)
//! render_case    -> CpuOracle::render(&scene, &render_spec(case, variant))
//!                -> corpus::straighten_alpha  (PNG is straight alpha)
//!                -> image::RgbaImage
//! ```
//!
//! Layout stays LOGICAL and the device scale composes at PAINT — [`crate::frame`]'s
//! split, inherited wholesale. That is why [`render_spec`] leaves
//! [`crate::render::RenderSpec::scale`] at `1.0`: [`FrameSpec::scale`] is already
//! baked into the recorded display list, and a second renderer-side scale would
//! apply it twice ([`crate::frame::physical_case`] makes the same choice for the
//! golden corpus).
//!
//! # Fidelity is reported, never silent
//!
//! [`crate::oracle_cpu::CpuOracle`] downgrades a handful of commands it has no
//! CPU equivalent for (a `ShaderQuad` becomes an opaque placeholder fill, an
//! image brush paints transparent, a `SceneTexture` draws nothing). The golden
//! gate treats any such downgrade as a failure; a website preview cannot, since
//! a shader case still needs *a* thumbnail. So the downgrade is recorded instead:
//! [`skip_labels`] turns [`crate::oracle_cpu::SkipReport`] into the strings
//! [`SnapshotEntry::skips`] carries into `manifest.json`, where a reader can see
//! exactly which previews are not faithful.
//!
//! # Fonts
//!
//! [`snapshot_text_context`] registers the bundled [`crate::fonts`] faces, so a
//! case whose text lies inside their subsets shapes identically everywhere. It
//! deliberately does NOT pin the theme's type scale onto those faces the way
//! [`crate::frame::pin_type_scale`] does for the golden corpus: the bundled Latin
//! face is a subset of eight codepoints, and forcing a real gallery label
//! (`"Click me"`) through it would render a preview of tofu. Gallery text
//! therefore reaches the host font collection, which makes these PNGs
//! host-dependent in exactly the way `docs/TESTING.md`'s Deterministic Inputs
//! forbids for a *baseline* — and is why they are not one.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use frust_core::FrameTime;
use frust_gallery::{Case, Design, Variant};
use frust_scene::Scene;
use frust_text::TextContext;
use image::RgbaImage;
use kurbo::Affine;
use peniko::Color;
use serde::{Deserialize, Serialize};

use crate::corpus::straighten_alpha;
use crate::fonts::register_test_fonts;
use crate::frame::{FrameSpec, record_view};
use crate::oracle_cpu::{CpuOracle, SkipReport};
use crate::render::{BackendMeta, RenderSpec, RenderedImage, SceneRenderer};

/// Every [`Variant`] a case is snapshotted under, in the order the manifest
/// lists them (and the order the two files per case are written in).
pub const VARIANTS: [Variant; 2] = [Variant::Light, Variant::Dark];

/// The recorded frame's identity when no `git` revision could be resolved.
pub const UNKNOWN_REVISION: &str = "unknown";

/// A [`TextContext`] carrying the bundled [`crate::fonts`] faces.
///
/// See the module docs' Fonts section for why this is *registration only* and
/// not the full [`crate::frame::pin_type_scale`] treatment the golden corpus
/// gets.
#[must_use]
pub fn snapshot_text_context() -> TextContext {
    let mut cx = TextContext::new();
    let _registered = register_test_fonts(&mut cx);
    cx
}

/// The deterministic frame inputs `case` records under: its own logical
/// viewport, its own device scale, and its own fixed timestamp
/// ([`Case::time_ms`], converted to the [`FrameTime`] nanosecond clock an
/// animating widget differences itself against).
#[must_use]
pub fn frame_spec(case: &Case) -> FrameSpec {
    FrameSpec {
        size: case.size,
        scale: case.scale,
        time: FrameTime::from_nanos(case.time_ms.saturating_mul(1_000_000)),
    }
}

/// The colour the target is cleared to before `case` is drawn: the active
/// [`frust_theme::ColorScheme::surface`] of the theme `variant` resolves to.
///
/// Not white. A gallery case paints only its own widget — nothing fills the
/// page behind it — so a fixed white base would put every `Dark` preview's
/// light-on-dark widget on a white card. Taking the surface token instead means
/// the two variants of a case differ exactly the way the website's own light and
/// dark pages do.
#[must_use]
pub fn base_color(case: &Case, variant: Variant) -> Color {
    frust_gallery::theme(case.design, variant).scheme().surface
}

/// The renderer-side spec for `case`: the PHYSICAL pixel size
/// [`frame_spec`]'s logical-size/scale pair implies, the variant's
/// [`base_color`], and **no second scale** (see the module docs).
#[must_use]
pub fn render_spec(case: &Case, variant: Variant) -> RenderSpec {
    let spec = frame_spec(case);
    RenderSpec {
        width: spec.physical_width(),
        height: spec.physical_height(),
        base_color: base_color(case, variant),
        scale: 1.0,
        root: Affine::IDENTITY,
    }
}

/// Records `case` under `variant` into a fresh [`Scene`] — a real
/// [`frust_core::RenderRoot`] rebuild, layout-with-text and paint, with no
/// window, no shell, no GPU and no clock (see [`crate::frame::record_view`]).
#[must_use]
pub fn record_scene(case: &Case, variant: Variant, fonts: &mut TextContext) -> Scene {
    let mut scene = Scene::new();
    record_view(
        &mut scene,
        &frame_spec(case),
        frust_gallery::theme(case.design, variant),
        fonts,
        case.build,
    );
    scene
}

/// A frame's pixels as straight-alpha RGBA, ready to encode as a PNG.
///
/// PNG is a straight-alpha format, so a premultiplied backend frame is
/// converted through [`crate::corpus::straighten_alpha`] first — the same,
/// deliberately lossy, one-way conversion the golden pipeline stores its
/// baselines through.
///
/// # Errors
///
/// Fails when the frame's byte count does not match its own declared
/// dimensions, which would otherwise be a silently truncated image.
pub fn to_rgba_image(image: &RenderedImage) -> Result<RgbaImage> {
    let straight = straighten_alpha(image);
    let (width, height) = (straight.width, straight.height);
    RgbaImage::from_raw(width, height, straight.rgba8).ok_or_else(|| {
        anyhow!("a {width}x{height} frame did not carry width * height * 4 bytes of RGBA")
    })
}

/// One rendered snapshot: the image, the fidelity gaps the backend reported
/// while producing it, and which backend that was.
#[derive(Clone, Debug)]
pub struct CaseRender {
    /// Straight-alpha RGBA pixels at the case's PHYSICAL size.
    pub image: RgbaImage,
    /// The backend's reported fidelity gaps, as [`skip_labels`] strings. Empty
    /// means the frame is a faithful render of the case.
    pub skips: Vec<String>,
    /// Which backend produced the frame — the provenance `manifest.json`
    /// records.
    pub backend: BackendMeta,
}

/// Renders `case` under `variant` on the GPU-free [`CpuOracle`].
///
/// The default path of the `widget-snapshots` binary, and the only one this
/// module contains: the optional `--gpu` arm lives in the binary, so nothing in
/// this library reaches for an adapter.
///
/// # Errors
///
/// Propagates the oracle's own render error (a zero/oversized target, a dash
/// pattern it refuses), with the case and variant named.
pub fn render_case(case: &Case, variant: Variant, fonts: &mut TextContext) -> Result<CaseRender> {
    let scene = record_scene(case, variant, fonts);
    let spec = render_spec(case, variant);
    let mut oracle = CpuOracle::new();
    let rendered = oracle.render(&scene, &spec).with_context(|| {
        format!(
            "rendering case {:?} ({}) on the CPU oracle",
            case.slug,
            variant_tag(variant)
        )
    })?;
    let skips = skip_labels(&oracle.skips());
    let image = to_rgba_image(&rendered)
        .with_context(|| format!("decoding the rendered frame for case {:?}", case.slug))?;
    Ok(CaseRender {
        image,
        skips,
        backend: oracle.meta(),
    })
}

/// A [`SkipReport`]'s non-zero counters as `name=count` strings, in a fixed
/// order.
///
/// Machine-readable on purpose: these land verbatim in
/// [`SnapshotEntry::skips`], which the `--check` comparison diffs, so a prose
/// rendering that changed wording would look like a content change.
#[must_use]
pub fn skip_labels(skips: &SkipReport) -> Vec<String> {
    [
        ("shader_quads", skips.shader_quads),
        ("image_brush_fills", skips.image_brush_fills),
        ("unrenderable_glyph_runs", skips.unrenderable_glyph_runs),
        ("unrenderable_images", skips.unrenderable_images),
        ("scene_textures", skips.scene_textures),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(label, count)| format!("{label}={count}"))
    .collect()
}

/// The manifest/filename tag for a [`Variant`] — the `.light`/`.dark` infix of
/// a snapshot's filename.
#[must_use]
pub fn variant_tag(variant: Variant) -> &'static str {
    match variant {
        Variant::Light => "light",
        Variant::Dark => "dark",
    }
}

/// The manifest tag for a [`Design`] — the same lowercase name the slug rule
/// prefixes a design system's own cases with (`material/button`, …).
#[must_use]
pub fn design_tag(design: Design) -> &'static str {
    match design {
        Design::Base => "base",
        Design::Material => "material",
        Design::Cupertino => "cupertino",
        Design::Glyph => "glyph",
        Design::Shadcn => "shadcn",
        Design::Beui => "beui",
    }
}

/// The output path a case's snapshot is written to, relative to the output
/// directory: `<slug>.<variant>.png`, with every `/` in the slug becoming a
/// subdirectory (`material/button` -> `material/button.light.png`).
///
/// # Errors
///
/// Refuses a slug that is not a safe relative path — an empty segment, or any
/// character outside `[A-Za-z0-9_-]`. That charset excludes `.`, so no segment
/// can be `.` or `..` and no slug can escape the output directory: this
/// function turns registry data into a filesystem path, and the registry is
/// editable by anyone adding a case.
pub fn snapshot_relative_path(slug: &str, variant: Variant) -> Result<PathBuf> {
    let segments: Vec<&str> = slug.split('/').collect();
    for segment in &segments {
        if segment.is_empty() {
            bail!("case slug {slug:?} has an empty path segment — a slug is `stem` or `dir/stem`");
        }
        if let Some(bad) = segment
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        {
            bail!(
                "case slug {slug:?} segment {segment:?} carries {bad:?}, outside the \
                 [A-Za-z0-9_-] a snapshot path may be built from"
            );
        }
    }
    let (stem, dirs) = segments
        .split_last()
        .expect("str::split always yields at least one segment");
    let mut path = PathBuf::new();
    for dir in dirs {
        path.push(dir);
    }
    path.push(format!("{stem}.{}.png", variant_tag(variant)));
    Ok(path)
}

/// One row of `manifest.json`: everything the website needs to place and label
/// a preview, plus the provenance that says whether it is a faithful render.
///
/// Sizes come in both units on purpose. `css_*` is the LOGICAL viewport the
/// case was laid out at — the size the `<img>` should occupy on the page —
/// and `px_*` is the physical size of the file, `css_* * `[`Case::scale`]
/// (2.0 by default, i.e. the @2x asset a 1x box displays).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// The case's [`Case::slug`] — also the website page's file stem, and the
    /// stem of this entry's own PNG (see [`snapshot_relative_path`]).
    pub slug: String,
    /// The case's human-readable [`Case::title`].
    pub title: String,
    /// The [`design_tag`] of the design system the case renders under.
    pub design: String,
    /// Logical (CSS) width the case was laid out at, rounded to whole pixels.
    pub css_width: u32,
    /// Logical (CSS) height the case was laid out at, rounded to whole pixels.
    pub css_height: u32,
    /// Physical width of the PNG.
    pub px_width: u32,
    /// Physical height of the PNG.
    pub px_height: u32,
    /// The [`variant_tag`] this row's PNG was rendered under.
    pub variant: String,
    /// Lowercase hex SHA-256 of the PNG file's bytes.
    pub sha256: String,
    /// The backend's fidelity gaps for this frame ([`skip_labels`]). Empty
    /// means faithful; non-empty means the preview shows a downgrade.
    pub skips: Vec<String>,
}

/// The whole `manifest.json`: the run's provenance plus one
/// [`SnapshotEntry`] per case x variant, in registry order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The frust commit the images were generated from, or
    /// [`UNKNOWN_REVISION`].
    pub frust_revision: String,
    /// RFC 3339 UTC timestamp of the run.
    pub generated_at: String,
    /// The rendering backend's stable id ([`crate::render::SceneRenderer::id`],
    /// or `headless-engine` for the binary's `--gpu` arm).
    pub backend: String,
    /// The backend's adapter identity — the CPU oracle's crate/version string,
    /// or the GPU adapter's reported name.
    pub adapter: String,
    /// One row per case x variant.
    pub snapshots: Vec<SnapshotEntry>,
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The manifest row for `case`/`variant`, given the PNG bytes that were
/// written for it and the backend's [`skip_labels`] for that frame.
#[must_use]
pub fn snapshot_entry(
    case: &Case,
    variant: Variant,
    png: &[u8],
    skips: Vec<String>,
) -> SnapshotEntry {
    let spec = frame_spec(case);
    SnapshotEntry {
        slug: case.slug.to_string(),
        title: case.title.to_string(),
        design: design_tag(case.design).to_string(),
        css_width: spec.size.width.round().max(0.0) as u32,
        css_height: spec.size.height.round().max(0.0) as u32,
        px_width: spec.physical_width(),
        px_height: spec.physical_height(),
        variant: variant_tag(variant).to_string(),
        sha256: sha256_hex(png),
        skips,
    }
}

/// The repository commit the snapshots are attributed to, or
/// [`UNKNOWN_REVISION`].
///
/// Shelled out to `git` rather than baked in at build time, for
/// [`crate::frame`]'s reason: an `env!`-style capture would freeze at the last
/// recompilation, which is exactly when it would be wrong. That module has a
/// private twin of this helper; it is not reused because hoisting it would mean
/// editing `frame.rs`, which is outside this feature's declared scope — a
/// follow-up should merge the two.
#[must_use]
pub fn frust_revision() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| UNKNOWN_REVISION.to_string())
}

/// The current UTC time as an RFC 3339 timestamp, for
/// [`Manifest::generated_at`].
///
/// Hand-rolled from [`std::time::SystemTime`] rather than pulling a date crate
/// in for one field (this repository's Version-Pin Policy makes a new
/// third-party pin the more expensive option); the civil-date conversion is
/// Howard Hinnant's `civil_from_days`, shifted so the era starts on 0000-03-01.
/// Always UTC. Same twin-in-`frame.rs` note as [`frust_revision`].
#[must_use]
pub fn generated_at() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Every way `fresh` disagrees with the manifest already `on_disk`, as
/// review-ready messages. Empty means the two describe the same images.
///
/// [`Manifest::frust_revision`] and [`Manifest::generated_at`] are deliberately
/// NOT compared: they are run provenance, they differ on every run by
/// construction, and a `--check` that failed on them would never pass.
/// Everything else is — including [`Manifest::backend`], since the same slug
/// rendered on a different backend is a different image.
///
/// `exhaustive` says whether `fresh` was meant to cover the whole registry. A
/// filtered run covers a subset, so rows present on disk and absent from
/// `fresh` are only reported as stale when the run was unfiltered.
#[must_use]
pub fn manifest_differences(on_disk: &Manifest, fresh: &Manifest, exhaustive: bool) -> Vec<String> {
    let mut out = Vec::new();
    if on_disk.backend != fresh.backend {
        out.push(format!(
            "manifest.json: backend {:?} on disk, {:?} in this run",
            on_disk.backend, fresh.backend
        ));
    }
    if on_disk.adapter != fresh.adapter {
        out.push(format!(
            "manifest.json: adapter {:?} on disk, {:?} in this run",
            on_disk.adapter, fresh.adapter
        ));
    }
    let key = |entry: &SnapshotEntry| (entry.slug.clone(), entry.variant.clone());
    for entry in &fresh.snapshots {
        match on_disk.snapshots.iter().find(|d| key(d) == key(entry)) {
            None => out.push(format!(
                "manifest.json: {}.{} is missing from the manifest on disk",
                entry.slug, entry.variant
            )),
            Some(disk_entry) if disk_entry != entry => out.push(format!(
                "manifest.json: {}.{} changed: {disk_entry:?} on disk, {entry:?} in this run",
                entry.slug, entry.variant
            )),
            Some(_) => {}
        }
    }
    if exhaustive {
        for entry in &on_disk.snapshots {
            if !fresh.snapshots.iter().any(|f| key(f) == key(entry)) {
                out.push(format!(
                    "manifest.json: {}.{} is on disk but no longer in the registry",
                    entry.slug, entry.variant
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_case() -> &'static Case {
        frust_gallery::find("button").expect("the seed `button` case is in the registry")
    }

    #[test]
    fn a_bare_slug_maps_to_a_flat_file_per_variant() {
        assert_eq!(
            snapshot_relative_path("button", Variant::Light).unwrap(),
            PathBuf::from("button.light.png")
        );
        assert_eq!(
            snapshot_relative_path("icon-button", Variant::Dark).unwrap(),
            PathBuf::from("icon-button.dark.png")
        );
    }

    #[test]
    fn a_design_prefixed_slug_maps_to_a_subdirectory() {
        assert_eq!(
            snapshot_relative_path("material/button", Variant::Light).unwrap(),
            PathBuf::from("material").join("button.light.png")
        );
        assert_eq!(
            snapshot_relative_path("a/b/c", Variant::Dark).unwrap(),
            PathBuf::from("a").join("b").join("c.dark.png")
        );
    }

    /// The slug is registry data turned into a filesystem path, so the escape
    /// shapes are refused rather than normalized.
    #[test]
    fn a_slug_that_could_escape_the_output_directory_is_refused() {
        for bad in [
            "../escape",
            "..",
            ".",
            "/absolute",
            "trailing/",
            "double//slash",
            "",
            "with space",
            "back\\slash",
            "dot.stem",
        ] {
            assert!(
                snapshot_relative_path(bad, Variant::Light).is_err(),
                "slug {bad:?} must be refused as a snapshot path"
            );
        }
    }

    #[test]
    fn the_render_spec_is_physical_and_carries_no_second_scale() {
        let case = a_case();
        let spec = render_spec(case, Variant::Light);
        assert_eq!(spec.width, (case.size.width * case.scale).round() as u32);
        assert_eq!(spec.height, (case.size.height * case.scale).round() as u32);
        assert_eq!(
            spec.scale, 1.0,
            "the device scale is already inside the recorded scene"
        );
        assert_eq!(spec.root, Affine::IDENTITY);
    }

    #[test]
    fn the_two_variants_of_a_case_clear_to_different_surfaces() {
        let case = a_case();
        assert_ne!(
            base_color(case, Variant::Light),
            base_color(case, Variant::Dark),
            "a light and a dark preview must not share a background"
        );
    }

    #[test]
    fn the_frame_time_comes_from_the_case_not_a_clock() {
        let mut case = *a_case();
        case.time_ms = 250;
        assert_eq!(
            frame_spec(&case).time,
            FrameTime::from_nanos(250_000_000),
            "Case::time_ms is milliseconds; FrameTime counts nanoseconds"
        );
    }

    #[test]
    fn only_non_zero_skip_counters_are_reported_and_the_order_is_fixed() {
        assert!(skip_labels(&SkipReport::default()).is_empty());
        let skips = SkipReport {
            shader_quads: 2,
            scene_textures: 1,
            ..SkipReport::default()
        };
        assert_eq!(
            skip_labels(&skips),
            vec!["shader_quads=2", "scene_textures=1"]
        );
    }

    #[test]
    fn sha256_is_lowercase_hex_of_the_bytes() {
        // The canonical empty-input SHA-256 digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sha256_hex(b"frust").len(), 64);
    }

    #[test]
    fn a_manifest_entry_serializes_the_documented_schema() {
        let case = a_case();
        let entry = snapshot_entry(case, Variant::Dark, b"not really a png", vec![]);
        let json = serde_json::to_value(&entry).expect("a SnapshotEntry serializes");
        let object = json.as_object().expect("an entry is a JSON object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "css_height",
                "css_width",
                "design",
                "px_height",
                "px_width",
                "sha256",
                "skips",
                "slug",
                "title",
                "variant",
            ]
        );
        assert_eq!(object["slug"], "button");
        assert_eq!(object["title"], "Button");
        assert_eq!(object["design"], "base");
        assert_eq!(object["variant"], "dark");
        assert_eq!(object["css_width"], 360);
        assert_eq!(object["css_height"], 240);
        assert_eq!(object["px_width"], 720);
        assert_eq!(object["px_height"], 480);
        assert_eq!(object["sha256"], sha256_hex(b"not really a png"));
        assert_eq!(object["skips"], serde_json::json!([]));
    }

    #[test]
    fn a_manifest_serializes_its_provenance_alongside_the_rows() {
        let manifest = Manifest {
            frust_revision: "deadbeef".to_string(),
            generated_at: "2026-09-07T00:00:00Z".to_string(),
            backend: crate::oracle_cpu::ORACLE_ID.to_string(),
            adapter: "vello_cpu".to_string(),
            snapshots: vec![snapshot_entry(a_case(), Variant::Light, b"png", vec![])],
        };
        let json = serde_json::to_value(&manifest).expect("a Manifest serializes");
        let object = json.as_object().expect("a manifest is a JSON object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "adapter",
                "backend",
                "frust_revision",
                "generated_at",
                "snapshots"
            ]
        );
        assert_eq!(object["snapshots"].as_array().map(Vec::len), Some(1));

        let round_tripped: Manifest =
            serde_json::from_value(json).expect("a Manifest round-trips through JSON");
        assert_eq!(round_tripped, manifest);
    }

    fn manifest_of(entries: Vec<SnapshotEntry>) -> Manifest {
        Manifest {
            frust_revision: "deadbeef".to_string(),
            generated_at: "2026-09-07T00:00:00Z".to_string(),
            backend: "cpu".to_string(),
            adapter: "oracle".to_string(),
            snapshots: entries,
        }
    }

    #[test]
    fn provenance_alone_is_never_a_manifest_difference() {
        let case = a_case();
        let rows = vec![snapshot_entry(case, Variant::Light, b"png", vec![])];
        let on_disk = manifest_of(rows.clone());
        let fresh = Manifest {
            frust_revision: "0123456".to_string(),
            generated_at: "2099-01-01T00:00:00Z".to_string(),
            ..manifest_of(rows)
        };
        assert!(manifest_differences(&on_disk, &fresh, true).is_empty());
    }

    #[test]
    fn a_changed_row_a_missing_row_and_a_stale_row_are_all_reported() {
        let case = a_case();
        let light = snapshot_entry(case, Variant::Light, b"png", vec![]);
        let dark = snapshot_entry(case, Variant::Dark, b"png", vec![]);
        let changed_light = snapshot_entry(case, Variant::Light, b"different", vec![]);

        // A row whose bytes moved.
        let differences = manifest_differences(
            &manifest_of(vec![light.clone()]),
            &manifest_of(vec![changed_light]),
            true,
        );
        assert_eq!(differences.len(), 1, "{differences:?}");
        assert!(differences[0].contains("button.light"));

        // A row the disk manifest has never seen.
        let differences = manifest_differences(
            &manifest_of(vec![]),
            &manifest_of(vec![light.clone()]),
            true,
        );
        assert_eq!(differences.len(), 1, "{differences:?}");
        assert!(differences[0].contains("missing from the manifest on disk"));

        // A row the registry no longer produces — reported only when the run
        // covered the whole registry.
        let stale = manifest_differences(
            &manifest_of(vec![light.clone(), dark]),
            &manifest_of(vec![light.clone()]),
            true,
        );
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].contains("no longer in the registry"));
        assert!(
            manifest_differences(
                &manifest_of(vec![
                    light.clone(),
                    snapshot_entry(case, Variant::Dark, b"png", vec![])
                ]),
                &manifest_of(vec![light]),
                false,
            )
            .is_empty(),
            "a filtered run must not report the rows it deliberately skipped"
        );
    }

    #[test]
    fn a_seed_case_renders_at_its_physical_size_on_the_cpu_oracle() {
        let case = a_case();
        let mut fonts = snapshot_text_context();
        let rendered =
            render_case(case, Variant::Light, &mut fonts).expect("the seed case renders");
        assert_eq!(rendered.image.width(), 720);
        assert_eq!(rendered.image.height(), 480);
        assert_eq!(rendered.backend.backend, crate::oracle_cpu::ORACLE_ID);
    }

    /// The `--check` contract, in the small: the same case rendered twice on
    /// the same host must produce the same pixels.
    #[test]
    fn rendering_the_same_case_twice_produces_the_same_pixels() {
        let case = a_case();
        let mut fonts = snapshot_text_context();
        let first = render_case(case, Variant::Dark, &mut fonts).expect("first render");
        let second = render_case(case, Variant::Dark, &mut fonts).expect("second render");
        assert_eq!(first.image.as_raw(), second.image.as_raw());
    }
}
