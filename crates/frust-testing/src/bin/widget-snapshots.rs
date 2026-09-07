//! `widget-snapshots` — render every `frust-gallery` case to a light and a dark
//! PNG plus a `manifest.json`, for the website's `static/preview/` tree.
//!
//! ```text
//! widget-snapshots --out DIR [--filter SUBSTR] [--list] [--check] [--gpu]
//! ```
//!
//! - `--out DIR` — where the tree is written (or, with `--check`, compared
//!   against). Required unless `--list` is given.
//! - `--filter SUBSTR` — restrict the run to cases whose slug contains
//!   `SUBSTR`.
//! - `--list` — print the matching cases (`slug`, `design`, `title`) and exit,
//!   rendering nothing.
//! - `--check` — render nothing to disk: re-render every matching case, compare
//!   the encoded bytes against the files already in `DIR`, and exit `1` listing
//!   every file that differs or is missing. This is the determinism gate.
//! - `--gpu` — render through `frust_render::HeadlessRenderer` (a real adapter)
//!   instead of the CPU oracle. Host-only and entirely opt-in: without it this
//!   binary never asks for a GPU.
//!
//! `scripts/widget-snapshots.sh` is the wrapper that runs this under the pinned
//! toolchain and prints the rsync line for the website.
//!
//! # `manifest.json` and `--filter`
//!
//! Every row is keyed by `(slug, variant)`. A `--filter` run's own manifest
//! only covers the cases it rendered, so writing it is never a plain
//! overwrite of `DIR/manifest.json`: the fresh rows are upserted into
//! whatever is already there ([`frust_testing::snapshot::merge_manifest`]) —
//! replacing the rows for the slugs this run touched, leaving every other row
//! exactly as it was. A run with no `--filter` is EXHAUSTIVE and writes a
//! full manifest describing the whole registry outright, no merge involved.
//! If `--out` holds no `manifest.json` yet, a filtered run cannot merge into
//! anything, so it writes one describing only the rows it rendered and says
//! so on stderr. `--check` makes the same exhaustive/filtered distinction —
//! see [`frust_testing::snapshot::manifest_differences`]'s `exhaustive`
//! parameter and the stray-PNG scan below.
//!
//! Each row's `skips` field is [`frust_testing::snapshot::SnapshotEntry::skips`]:
//! on the CPU oracle arm (the default), an empty list is a verified claim of
//! fidelity. The `--gpu` arm has no equivalent report to ask, so it writes
//! the single marker `"unknown:gpu-backend"` instead of an empty list — a
//! website consumer of this schema must not read an empty `skips` on a
//! `--gpu`-backed row as "faithful"; it means "never checked".
//!
//! # Why `--check` holds no temp directory
//!
//! The comparison is on the *encoded PNG bytes*, which the run has in memory
//! before it would ever write them, so there is nothing a scratch directory
//! would add except files to clean up (and a second, differently-failing way to
//! fail). `--check` therefore renders, encodes, and diffs the bytes against
//! `DIR` without writing anything anywhere.

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use frust_gallery::{Case, Variant};
use frust_render::{HeadlessOptions, HeadlessRenderer, HeadlessSpec};
use frust_testing::snapshot::{
    CaseRender, Manifest, SnapshotEntry, VARIANTS, frust_revision, generated_at,
    manifest_differences, merge_manifest, record_scene, render_spec, snapshot_entry,
    snapshot_relative_path, snapshot_text_context, to_rgba_image, variant_tag,
};
use frust_testing::{AlphaKind, BackendMeta, RenderedImage};
use frust_text::TextContext;
use image::{ImageFormat, RgbaImage};

/// The `backend` id `manifest.json` records for the `--gpu` arm — the CPU arm
/// records `frust_testing::ORACLE_ID` instead, straight off the oracle.
const GPU_BACKEND_ID: &str = "headless-engine";

/// The single [`SnapshotEntry::skips`] marker `render_on_gpu` writes for
/// every row it produces: the `--gpu` arm has no `SkipReport` counterpart to
/// ask, so an empty `skips` there would claim a fidelity guarantee this arm
/// cannot make (see the module docs' `manifest.json` and `--filter` section).
const GPU_SKIPS_UNKNOWN: &str = "unknown:gpu-backend";

/// The manifest filename, alongside the PNGs, in the output directory root.
const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, Parser)]
#[command(
    name = "widget-snapshots",
    about = "Render every frust-gallery case to light/dark PNGs plus a manifest.json",
    long_about = None,
)]
struct Cli {
    /// Directory the PNG tree and manifest.json are written to (or, with
    /// --check, compared against). An exhaustive write (no --filter) also
    /// DELETES every PNG that the manifest.json already in DIR lists but this
    /// run no longer produces (a renamed or removed case); files the manifest
    /// does not list are never touched. Point it at a disposable build
    /// directory such as target/widget-snapshots.
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// Only render cases whose slug contains this substring.
    #[arg(long, value_name = "SUBSTR")]
    filter: Option<String>,
    /// List the matching cases and exit without rendering.
    #[arg(long)]
    list: bool,
    /// Re-render and byte-compare against --out instead of writing; exit 1 on
    /// any difference.
    #[arg(long)]
    check: bool,
    /// Render through a real GPU adapter (frust_render::HeadlessRenderer)
    /// rather than the GPU-free CPU oracle.
    #[arg(long)]
    gpu: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("widget-snapshots: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    let cases: Vec<&'static Case> = frust_gallery::cases()
        .iter()
        .filter(|case| {
            cli.filter
                .as_ref()
                .is_none_or(|needle| case.slug.contains(needle.as_str()))
        })
        .collect();

    if cli.list {
        for case in &cases {
            println!(
                "{}\t{}\t{}",
                case.slug,
                frust_testing::snapshot::design_tag(case.design),
                case.title
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    if cases.is_empty() {
        match &cli.filter {
            Some(needle) => bail!(
                "no case slug contains {needle:?} — `--list` prints the {} case(s) in the registry",
                frust_gallery::cases().len()
            ),
            None => bail!("the frust-gallery registry is empty — there is nothing to render"),
        }
    }

    let out = cli
        .out
        .clone()
        .context("--out DIR is required (only --list may omit it)")?;

    let mut backend = Backend::open(cli.gpu)?;
    let mut fonts = snapshot_text_context();

    // Render and encode everything first: a run that would fail on its last
    // case must not leave a half-written tree behind.
    let mut rendered: Vec<(PathBuf, Vec<u8>, SnapshotEntry)> = Vec::new();
    let mut backend_meta: Option<BackendMeta> = None;
    for case in &cases {
        for variant in VARIANTS {
            let relative = snapshot_relative_path(case.slug, variant)?;
            let frame = backend.render(case, variant, &mut fonts)?;
            let png = encode_png(&frame.image).with_context(|| {
                format!("encoding {}.{} as a PNG", case.slug, variant_tag(variant))
            })?;
            let entry = snapshot_entry(case, variant, &png, frame.skips.clone());
            if !frame.skips.is_empty() {
                eprintln!(
                    "widget-snapshots: {}.{} is not a faithful render — the backend downgraded \
                     it ({})",
                    case.slug,
                    variant_tag(variant),
                    frame.skips.join(", ")
                );
            }
            backend_meta.get_or_insert(frame.backend);
            rendered.push((relative, png, entry));
        }
    }

    let meta = backend_meta.expect("a non-empty case list renders at least one frame");
    let manifest = Manifest {
        frust_revision: frust_revision(),
        generated_at: generated_at(),
        backend: meta.backend.clone(),
        adapter: meta.adapter.clone(),
        snapshots: rendered.iter().map(|(_, _, entry)| entry.clone()).collect(),
    };

    let exhaustive = cli.filter.is_none();

    if cli.check {
        check(&out, &rendered, &manifest, exhaustive)
    } else {
        // Read the manifest this run is about to replace BEFORE writing: on an
        // exhaustive run it is the only record of which PNGs under `--out`
        // this tool produced earlier, and therefore the only set it may prune.
        let previous = if exhaustive {
            read_existing_manifest(&out)?
        } else {
            None
        };
        let to_write = if exhaustive {
            manifest
        } else {
            merge_with_existing(&out, manifest)?
        };
        write(&out, &rendered, &to_write)?;
        if let Some(previous) = previous {
            let pruned = prune_stale(&out, &previous, &rendered)?;
            for path in &pruned {
                eprintln!(
                    "widget-snapshots: pruned {} (listed by the previous manifest, no longer produced)",
                    path.display()
                );
            }
            if !pruned.is_empty() {
                eprintln!(
                    "widget-snapshots: pruned {} stale PNG(s) from {}",
                    pruned.len(),
                    out.display()
                );
            }
        }
        eprintln!(
            "widget-snapshots: wrote {} PNG(s) + {MANIFEST_NAME} to {} ({} on {})",
            rendered.len(),
            out.display(),
            meta.backend,
            meta.adapter
        );
        Ok(ExitCode::SUCCESS)
    }
}

/// Resolves the manifest a `--filter` run should write: `fresh` merged onto
/// whatever `manifest.json` is already at `out` ([`merge_manifest`]), or
/// `fresh` as-is when there is nothing to merge into.
///
/// # Errors
///
/// Propagates a parse failure on an existing-but-unreadable manifest, and
/// [`merge_manifest`]'s own refusal on a backend mismatch.
fn merge_with_existing(out: &Path, fresh: Manifest) -> Result<Manifest> {
    let manifest_path = out.join(MANIFEST_NAME);
    match fs::read_to_string(&manifest_path) {
        Ok(text) => {
            let on_disk: Manifest = serde_json::from_str(&text)
                .with_context(|| format!("parsing existing {}", manifest_path.display()))?;
            merge_manifest(&on_disk, &fresh)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "widget-snapshots: no existing {MANIFEST_NAME} at {} — writing one that covers \
                 only this filtered run's {} case(s)",
                out.display(),
                fresh.snapshots.len()
            );
            Ok(fresh)
        }
        Err(err) => {
            Err(err).with_context(|| format!("reading existing {}", manifest_path.display()))
        }
    }
}

/// Refuses to write through `path` when it already exists as a symlink — a
/// case, or design-prefixed slug directory, replaced by a link would let a
/// write land somewhere outside `--out` entirely. A path that does not exist
/// yet (the ordinary case: `create_dir_all` is about to make it) is not a
/// symlink and is not refused.
///
/// [`std::fs::symlink_metadata`] rather than [`std::path::Path::exists`]/
/// [`std::fs::metadata`] on purpose: those follow a symlink to ask about its
/// target, which is exactly the property a symlink swap exploits.
fn refuse_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("{}: refusing to write through a symlink", path.display())
        }
        _ => Ok(()),
    }
}

/// Writes the whole tree: every PNG under its slug-derived path, then the
/// manifest.
fn write(
    out: &Path,
    rendered: &[(PathBuf, Vec<u8>, SnapshotEntry)],
    manifest: &Manifest,
) -> Result<()> {
    refuse_symlink(out)?;
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    for (relative, png, _) in rendered {
        let path = out.join(relative);
        if let Some(parent) = path.parent() {
            refuse_symlink(parent)?;
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        refuse_symlink(&path)?;
        fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
    }
    let mut json = serde_json::to_string_pretty(manifest).context("serializing the manifest")?;
    json.push('\n');
    let path = out.join(MANIFEST_NAME);
    refuse_symlink(&path)?;
    fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

/// The `manifest.json` already at `out`, or `None` when there is none yet.
///
/// # Errors
///
/// A manifest that exists but cannot be read or parsed is an error, never
/// silently treated as absent: it is the record of what this tool wrote
/// earlier, and a corrupt one must not be papered over.
fn read_existing_manifest(out: &Path) -> Result<Option<Manifest>> {
    let manifest_path = out.join(MANIFEST_NAME);
    match fs::read_to_string(&manifest_path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("parsing existing {}", manifest_path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => {
            Err(err).with_context(|| format!("reading existing {}", manifest_path.display()))
        }
    }
}

/// After an EXHAUSTIVE write, removes the PNGs that `previous` (the
/// `manifest.json` this run just replaced) listed but this run no longer
/// produced — a case renamed or deleted from the registry — so a stale
/// snapshot cannot linger under `--out` and be published: `rsync -a --delete`
/// at the publish step only deletes what is absent from its *source*, and a
/// stale PNG still present under `--out` would be copied, not removed.
///
/// The delete set is bounded to files the previous manifest attributes to this
/// tool: a `.png` the manifest never listed is never touched, whatever
/// directory `--out` names (an operator who points `--out` at a populated tree
/// loses nothing the generator did not itself write). Symlinked entries are
/// refused rather than followed; a missing file is not an error (it was
/// already gone). A filtered run never prunes. Returns the removed paths.
fn prune_stale(
    out: &Path,
    previous: &Manifest,
    rendered: &[(PathBuf, Vec<u8>, SnapshotEntry)],
) -> Result<Vec<PathBuf>> {
    let expected: BTreeSet<PathBuf> = rendered
        .iter()
        .map(|(relative, _, _)| out.join(relative))
        .collect();
    let mut pruned = Vec::new();
    for row in &previous.snapshots {
        let Some(variant) = [Variant::Light, Variant::Dark]
            .into_iter()
            .find(|v| variant_tag(*v) == row.variant)
        else {
            continue;
        };
        let path = out.join(snapshot_relative_path(&row.slug, variant)?);
        if expected.contains(&path) {
            continue;
        }
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("{}: refusing to prune through a symlink", path.display())
            }
            Ok(meta) if meta.is_file() => {
                fs::remove_file(&path)
                    .with_context(|| format!("pruning stale {}", path.display()))?;
                pruned.push(path);
            }
            _ => {}
        }
    }
    pruned.sort();
    Ok(pruned)
}

/// Compares this run's bytes against the tree already in `out`, reporting every
/// missing or differing file rather than stopping at the first.
fn check(
    out: &Path,
    rendered: &[(PathBuf, Vec<u8>, SnapshotEntry)],
    manifest: &Manifest,
    exhaustive: bool,
) -> Result<ExitCode> {
    let mut failures: Vec<String> = Vec::new();

    for (relative, png, _) in rendered {
        let path = out.join(relative);
        match fs::read(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                failures.push(format!("{}: missing", path.display()));
            }
            Err(err) => failures.push(format!("{}: unreadable: {err}", path.display())),
            Ok(existing) if existing != *png => failures.push(format!(
                "{}: differs ({} byte(s) on disk, {} byte(s) in this run)",
                path.display(),
                existing.len(),
                png.len()
            )),
            Ok(_) => {}
        }
    }

    let manifest_path = out.join(MANIFEST_NAME);
    match fs::read_to_string(&manifest_path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            failures.push(format!("{}: missing", manifest_path.display()));
        }
        Err(err) => failures.push(format!("{}: unreadable: {err}", manifest_path.display())),
        Ok(text) => match serde_json::from_str::<Manifest>(&text) {
            Err(err) => failures.push(format!("{}: unparseable: {err}", manifest_path.display())),
            Ok(on_disk) => failures.extend(manifest_differences(&on_disk, manifest, exhaustive)),
        },
    }

    // A stray PNG under `out` that no case produces is a stale asset the
    // website would keep serving; only an unfiltered run can tell.
    if exhaustive {
        let expected: BTreeSet<PathBuf> = rendered
            .iter()
            .map(|(relative, _, _)| out.join(relative))
            .collect();
        let strays = stray_pngs(out, &expected);
        for stray in &strays.pngs {
            failures.push(format!("{}: not produced by any case", stray.display()));
        }
        for symlink in &strays.symlinks {
            failures.push(format!(
                "{}: symlinked entry refused under --check",
                symlink.display()
            ));
        }
    }

    if failures.is_empty() {
        eprintln!(
            "widget-snapshots: {} PNG(s) + {MANIFEST_NAME} in {} match this run",
            rendered.len(),
            out.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!(
        "widget-snapshots: {} difference(s) against {}:",
        failures.len(),
        out.display()
    );
    for failure in &failures {
        eprintln!("  {failure}");
    }
    Ok(ExitCode::FAILURE)
}

/// [`stray_pngs`]'s result: strays it found, plus every symlinked entry it
/// refused to follow rather than silently treating as a directory or a file.
#[derive(Debug, Default)]
struct StrayScan {
    /// Every `.png` under the root that is not in the run's expected set,
    /// sorted.
    pngs: Vec<PathBuf>,
    /// Every symlink the scan encountered (file or directory), sorted. A
    /// symlinked directory is never descended into, and a symlinked `.png`
    /// is never counted as a stray — either would mean trusting whatever the
    /// link points at instead of what is really under `--out`.
    symlinks: Vec<PathBuf>,
}

/// Every `.png` under `root` that is not in `expected`, sorted — plus any
/// symlink the walk refused to follow ([`StrayScan::symlinks`]).
///
/// A read error is treated as "no strays here" rather than a failure: this is a
/// staleness hint over a directory the caller may not have created yet, and the
/// missing-file checks above already speak for anything that matters.
///
/// Uses [`fs::symlink_metadata`] rather than [`Path::is_dir`] (which follows
/// a symlink to ask about its target): a symlinked directory swapped in under
/// `--out` must not be walked into, and a symlinked `.png` must not be
/// reported as an ordinary stray file — both are refused instead.
fn stray_pngs(root: &Path, expected: &BTreeSet<PathBuf>) -> StrayScan {
    let mut result = StrayScan::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                result.symlinks.push(path);
            } else if meta.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "png") && !expected.contains(&path)
            {
                result.pngs.push(path);
            }
        }
    }
    result.pngs.sort();
    result.symlinks.sort();
    result
}

/// PNG bytes for `image`, encoded in memory so `--check` and the write path
/// compare and store exactly the same bytes.
fn encode_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .context("encoding RGBA pixels as a PNG")?;
    Ok(bytes)
}

/// Which renderer this run draws through.
///
/// The CPU arm holds no state (the oracle is built per frame inside
/// `frust_testing::snapshot::render_case`); the GPU arm holds one adapter,
/// device and engine renderer for the whole run — and is only ever constructed
/// when `--gpu` was passed, so a default run asks for no adapter at all.
enum Backend {
    Cpu,
    Gpu(Box<HeadlessRenderer>),
}

impl Backend {
    fn open(gpu: bool) -> Result<Self> {
        if !gpu {
            return Ok(Self::Cpu);
        }
        let renderer = pollster::block_on(HeadlessRenderer::new(HeadlessOptions::default()))
            .context("--gpu: opening a headless GPU renderer")?;
        eprintln!("widget-snapshots: --gpu renderer is {}", renderer.meta());
        Ok(Self::Gpu(Box::new(renderer)))
    }

    fn render(
        &mut self,
        case: &Case,
        variant: Variant,
        fonts: &mut TextContext,
    ) -> Result<CaseRender> {
        match self {
            Self::Cpu => frust_testing::snapshot::render_case(case, variant, fonts),
            Self::Gpu(renderer) => render_on_gpu(renderer, case, variant, fonts),
        }
    }
}

/// The `--gpu` arm: the same scene and the same physical size as the CPU arm,
/// encoded by the real engine against a real adapter.
///
/// Plan OPEN #2 — this exists so a maintainer can eyeball the engine's own
/// output for the same case, not as a second source of website assets: an
/// adapter-dependent PNG is not something `--check` can hold to a byte, and no
/// `SkipReport` counterpart exists on this path, so [`CaseRender::skips`]
/// carries [`GPU_SKIPS_UNKNOWN`] rather than an empty vector that would claim
/// a fidelity guarantee this arm cannot make.
fn render_on_gpu(
    renderer: &mut HeadlessRenderer,
    case: &Case,
    variant: Variant,
    fonts: &mut TextContext,
) -> Result<CaseRender> {
    let scene = record_scene(case, variant, fonts);
    let spec = render_spec(case, variant);
    let headless = HeadlessSpec {
        width: spec.width,
        height: spec.height,
        base_color: spec.base_color,
        // `RenderSpec::scale` is 1.0 by construction (the device scale is
        // already in the scene), and `HeadlessSpec` has no scale field at all —
        // so the two arms rasterize the same geometry.
        root: spec.root,
    };
    let image = pollster::block_on(renderer.render(&scene, &headless)).with_context(|| {
        format!(
            "rendering case {:?} ({}) on the GPU",
            case.slug,
            variant_tag(variant)
        )
    })?;
    let meta = BackendMeta {
        backend: GPU_BACKEND_ID.to_string(),
        adapter: image.meta.adapter.clone(),
        driver: image.meta.driver.clone(),
        device_kind: image.meta.backend.clone(),
    };
    let rendered = RenderedImage {
        width: image.width,
        height: image.height,
        rgba8: image.rgba8,
        // The engine writes premultiplied alpha (`HeadlessImage::rgba8`);
        // `to_rgba_image` straightens it, because PNG is a straight-alpha
        // format.
        alpha: AlphaKind::Premultiplied,
        meta: meta.clone(),
    };
    Ok(CaseRender {
        image: to_rgba_image(&rendered)?,
        skips: vec![GPU_SKIPS_UNKNOWN.to_string()],
        backend: meta,
    })
}
