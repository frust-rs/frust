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
//!
//! # `manifest.json` under `--filter`
//!
//! A `--filter` run only renders a subset of the registry, so its own
//! [`Manifest`] only describes that subset. The binary never writes that
//! partial manifest over a full one on disk -- [`merge_manifest`] upserts the
//! fresh subset's rows into whatever `manifest.json` is already at `--out`
//! (replacing rows for the re-rendered slugs, keeping every other row
//! untouched) and refuses with a clear error if the two runs disagree on
//! [`Manifest::backend`], since a CPU-oracle row and a `--gpu` row are not
//! interchangeable data. Only an EXHAUSTIVE run (no `--filter`) writes a full
//! manifest outright; see [`manifest_differences`]'s `exhaustive` parameter
//! for the `--check` side of the same distinction. When `--out` holds no
//! `manifest.json` yet, a filtered run writes one that describes only its own
//! rows and says so on stderr -- there is nothing to merge into.
//!
//! Writing never prunes a stale `<slug>.<variant>.png` an exhaustive run no
//! longer produces (only `--check` flags one, via `stray_pngs`).
//! `scripts/widget-snapshots.sh` documents `rsync -a --delete` as the publish
//! step, which is where pruning actually happens -- the generator's own
//! `--out` tree is disposable build output, not the published one.

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
    /// The backend's fidelity gaps for this frame. On the CPU oracle arm this
    /// is [`skip_labels`]: empty genuinely means faithful (every command had
    /// a CPU equivalent), non-empty names which downgrade occurred. The
    /// `--gpu` arm (`render_on_gpu` in the binary) has no [`SkipReport`]
    /// counterpart to ask, so an empty vector there would silently claim a
    /// fidelity guarantee that arm cannot make; it instead writes the single
    /// marker `"unknown:gpu-backend"`, so a reader can tell "verified
    /// faithful" apart from "fidelity was never checked".
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

/// Upserts `fresh`'s rows into `on_disk`'s, for a `--filter` run: every row
/// `fresh` carries replaces the `on_disk` row of the same `(slug, variant)`
/// (or is appended, if `on_disk` never had one), and every `on_disk` row
/// `fresh` does not mention is kept untouched — see the module docs'
/// `manifest.json` under `--filter` section for why a filtered run must never
/// simply overwrite the manifest on disk.
///
/// The merged manifest's provenance ([`Manifest::frust_revision`],
/// [`Manifest::generated_at`], [`Manifest::adapter`]) is `fresh`'s: a merge is
/// still a real run, and the newest provenance is the most useful one to
/// record. [`Manifest::backend`] is `fresh`'s too, but only once refused
/// otherwise — [`Manifest::backend`] is not run provenance the way the other
/// three fields are: a CPU-oracle row and a `--gpu` row are not the same kind
/// of image, so merging one backend's fresh rows into another backend's
/// on-disk manifest would silently mislabel every untouched row.
///
/// # Errors
///
/// Refuses when `on_disk.backend != fresh.backend`.
pub fn merge_manifest(on_disk: &Manifest, fresh: &Manifest) -> Result<Manifest> {
    if on_disk.backend != fresh.backend {
        bail!(
            "manifest.json on disk was generated with backend {:?}, but this filtered run used \
             {:?} — merging would mislabel every row this run did not touch; re-run with \
             --gpu to match, or start a fresh --out directory",
            on_disk.backend,
            fresh.backend
        );
    }

    let key = |entry: &SnapshotEntry| (entry.slug.clone(), entry.variant.clone());
    let mut snapshots = on_disk.snapshots.clone();
    for entry in &fresh.snapshots {
        match snapshots.iter_mut().find(|d| key(d) == key(entry)) {
            Some(slot) => *slot = entry.clone(),
            None => snapshots.push(entry.clone()),
        }
    }

    Ok(Manifest {
        frust_revision: fresh.frust_revision.clone(),
        generated_at: fresh.generated_at.clone(),
        backend: fresh.backend.clone(),
        adapter: fresh.adapter.clone(),
        snapshots,
    })
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
    fn merge_manifest_upserts_the_fresh_rows_and_keeps_the_rest() {
        let case = a_case();
        let old_light = snapshot_entry(case, Variant::Light, b"old", vec![]);
        let dark = snapshot_entry(case, Variant::Dark, b"png", vec![]);
        let new_light = snapshot_entry(case, Variant::Light, b"new", vec![]);

        // `dark` is untouched by the filtered run; `light` is re-rendered.
        let on_disk = manifest_of(vec![old_light, dark.clone()]);
        let fresh = manifest_of(vec![new_light.clone()]);

        let merged = merge_manifest(&on_disk, &fresh).expect("same backend, must merge");
        assert_eq!(merged.frust_revision, fresh.frust_revision);
        assert_eq!(merged.generated_at, fresh.generated_at);
        assert_eq!(merged.adapter, fresh.adapter);
        assert_eq!(merged.backend, fresh.backend);

        let mut keys: Vec<(&str, &str)> = merged
            .snapshots
            .iter()
            .map(|e| (e.slug.as_str(), e.variant.as_str()))
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec![("button", "dark"), ("button", "light")]);
        assert!(
            merged.snapshots.contains(&dark),
            "the untouched row survives"
        );
        assert!(
            merged.snapshots.contains(&new_light),
            "the re-rendered row's new bytes win, not the stale on-disk ones"
        );
    }

    #[test]
    fn merge_manifest_appends_a_row_the_on_disk_manifest_never_had() {
        let case = a_case();
        let light = snapshot_entry(case, Variant::Light, b"png", vec![]);
        let dark = snapshot_entry(case, Variant::Dark, b"png", vec![]);

        let on_disk = manifest_of(vec![light.clone()]);
        let fresh = manifest_of(vec![dark.clone()]);
        let merged = merge_manifest(&on_disk, &fresh).expect("same backend, must merge");

        assert_eq!(merged.snapshots.len(), 2);
        assert!(merged.snapshots.contains(&light));
        assert!(merged.snapshots.contains(&dark));
    }

    #[test]
    fn merge_manifest_refuses_a_backend_mismatch() {
        let case = a_case();
        let row = snapshot_entry(case, Variant::Light, b"png", vec![]);
        let on_disk = Manifest {
            backend: "cpu-oracle".to_string(),
            ..manifest_of(vec![row.clone()])
        };
        let fresh = Manifest {
            backend: "headless-engine".to_string(),
            ..manifest_of(vec![row])
        };
        let err = merge_manifest(&on_disk, &fresh)
            .expect_err("a backend mismatch must be refused, never silently merged");
        let message = err.to_string();
        assert!(message.contains("cpu-oracle"), "{message}");
        assert!(message.contains("headless-engine"), "{message}");
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
