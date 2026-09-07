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
    manifest_differences, record_scene, render_spec, snapshot_entry, snapshot_relative_path,
    snapshot_text_context, to_rgba_image, variant_tag,
};
use frust_testing::{AlphaKind, BackendMeta, RenderedImage};
use frust_text::TextContext;
use image::{ImageFormat, RgbaImage};

/// The `backend` id `manifest.json` records for the `--gpu` arm — the CPU arm
/// records `frust_testing::ORACLE_ID` instead, straight off the oracle.
const GPU_BACKEND_ID: &str = "headless-engine";

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
    /// --check, compared against).
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

    if cli.check {
        check(&out, &rendered, &manifest, cli.filter.is_none())
    } else {
        write(&out, &rendered, &manifest)?;
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

/// Writes the whole tree: every PNG under its slug-derived path, then the
/// manifest.
fn write(
    out: &Path,
    rendered: &[(PathBuf, Vec<u8>, SnapshotEntry)],
    manifest: &Manifest,
) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    for (relative, png, _) in rendered {
        let path = out.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
    }
    let mut json = serde_json::to_string_pretty(manifest).context("serializing the manifest")?;
    json.push('\n');
    let path = out.join(MANIFEST_NAME);
    fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
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
        for stray in stray_pngs(out, &expected) {
            failures.push(format!("{}: not produced by any case", stray.display()));
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

/// Every `.png` under `root` that is not in `expected`, sorted.
///
/// A read error is treated as "no strays here" rather than a failure: this is a
/// staleness hint over a directory the caller may not have created yet, and the
/// missing-file checks above already speak for anything that matters.
fn stray_pngs(root: &Path, expected: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "png") && !expected.contains(&path)
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
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
/// `SkipReport` counterpart exists on this path, so [`CaseRender::skips`] comes
/// back empty rather than claiming a fidelity guarantee this arm cannot make.
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
        skips: Vec::new(),
        backend: meta,
    })
}
