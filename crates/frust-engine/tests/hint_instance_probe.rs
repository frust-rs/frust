//! Tripwire for the assumption behind `atlas_policy::embolden_basis`: that `glifo`
//! can always obtain a hinting instance for a run it treats as hinted.
//!
//! `glifo` 0.3.0 (`glyph.rs`, `HintKey::instance` and `HintCache::get`) builds a
//! `skrifa::outline::HintingInstance` with a fixed option set, and on failure
//! draws the run at `unitsPerEm` instead of the device size. The engine's faux-bold
//! stroke is measured in device pixels on the hinted path, so a face for which the
//! instance cannot be built would be emboldened by roughly `1/upem` of the intended
//! amount. This test builds the same instance (same options, both through `new` and
//! through the `reconfigure` path the `HintCache` takes when it recycles an entry)
//! for every face the workspace ships, over every size the engine uses (8..=96 px),
//! at the default location and at each variation axis extreme. Unhinted runs
//! (`hint(false)`) never build an instance and are not probed.
//!
//! `skrifa` is a dev-dependency only; the options below are a copy of `glifo`'s
//! private `HINTING_OPTIONS` and must follow it when `glifo` is bumped.

use std::path::{Path, PathBuf};

use skrifa::{
    FontRef, MetadataProvider,
    instance::Size,
    outline::{Engine, HintingInstance, HintingOptions, SmoothMode, Target},
};

const HINTING_OPTIONS: HintingOptions = HintingOptions {
    engine: Engine::AutoFallback,
    target: Target::Smooth {
        mode: SmoothMode::Lcd,
        symmetric_rendering: false,
        preserve_linear_metrics: true,
    },
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn collect_fonts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_fonts(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ttf" | "otf" | "ttc" | "otc")
        ) {
            out.push(path);
        }
    }
}

fn bundled_faces() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = Vec::new();
    collect_fonts(&root.join("testing/fonts"), &mut out);
    collect_fonts(&root.join("crates/frust-text/tests/fonts"), &mut out);
    if let Ok(plugins) = std::fs::read_dir(root.join("plugins")) {
        let mut plugins: Vec<_> = plugins.flatten().map(|e| e.path()).collect();
        plugins.sort();
        for plugin in plugins {
            collect_fonts(&plugin.join("fonts"), &mut out);
        }
    }
    out
}

/// Normalized coordinate sets to probe: default, then every axis at min and max.
fn locations(font: &FontRef<'_>) -> Vec<Vec<skrifa::instance::NormalizedCoord>> {
    let axes = font.axes();
    let mut sets = vec![Vec::new()];
    for axis in axes.iter() {
        for value in [axis.min_value(), axis.max_value()] {
            let location = axes.location([(axis.tag(), value)]);
            sets.push(location.coords().to_vec());
        }
    }
    sets
}

/// Probes every face in `bytes` (all collection members); returns failure strings.
fn probe(label: &str, bytes: &[u8]) -> (usize, Vec<String>) {
    let mut failures = Vec::new();
    let mut probed = 0;
    let count = skrifa::raw::FileRef::new(bytes).map_or(0, |f| match f {
        skrifa::raw::FileRef::Font(_) => 1,
        skrifa::raw::FileRef::Collection(c) => c.len(),
    });
    for index in 0..count {
        let Ok(font) = FontRef::from_index(bytes, index) else {
            continue;
        };
        let outlines = font.outline_glyphs();
        if outlines.format().is_none() {
            // No outline table skrifa reads (bitmap-only face): glifo never hints it.
            continue;
        }
        for coords in locations(&font) {
            let mut recycled: Option<HintingInstance> = None;
            for px in 8..=96u32 {
                let size = Size::new(px as f32);
                probed += 1;
                match HintingInstance::new(&outlines, size, coords.as_slice(), HINTING_OPTIONS) {
                    Ok(_) => {}
                    Err(e) => failures.push(format!("{label}#{index} new {px}px: {e}")),
                }
                match &mut recycled {
                    None => {
                        recycled = HintingInstance::new(
                            &outlines,
                            size,
                            coords.as_slice(),
                            HINTING_OPTIONS,
                        )
                        .ok();
                    }
                    Some(instance) => {
                        if let Err(e) = instance.reconfigure(
                            &outlines,
                            size,
                            coords.as_slice(),
                            HINTING_OPTIONS,
                        ) {
                            failures.push(format!("{label}#{index} reconfigure {px}px: {e}"));
                        }
                    }
                }
            }
        }
    }
    (probed, failures)
}

/// Probes every path; returns the failure lines and the number of probes made.
fn run(paths: &[PathBuf]) -> Vec<String> {
    let mut failures = Vec::new();
    let mut probed = 0;
    for path in paths {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let (n, f) = probe(&path.display().to_string(), &bytes);
        probed += n;
        failures.extend(f);
    }
    assert!(probed > 0, "no face was probed");
    failures
}

#[test]
fn every_bundled_face_yields_a_hinting_instance_at_every_engine_size() {
    let faces = bundled_faces();
    assert!(
        faces.len() >= 15,
        "expected the bundled faces, found {}",
        faces.len()
    );
    let failures = run(&faces);
    assert!(
        failures.is_empty(),
        "glifo cannot obtain a hinting instance for these faces; embolden_basis must fall back \
         to the upem basis for them:\n{}",
        failures.join("\n")
    );
}

/// macOS system faces on which `skrifa` cannot build a hinting instance (measured
/// 2026-11-02, macOS, skrifa 0.44.0): `glifo` draws these at `upem` when hinted.
/// The test pins the set exactly, so a face leaving or joining it is noticed.
const KNOWN_UNHINTABLE: &[&str] = &["Supplemental/Brush Script.ttf#0", "Fonts/Courier.ttc#2"];

#[test]
#[ignore = "macOS system faces; run with --ignored on a Mac"]
fn macos_system_faces_unhintable_set_is_pinned() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut faces = Vec::new();
    collect_fonts(Path::new("/System/Library/Fonts"), &mut faces);
    collect_fonts(Path::new("/System/Library/Fonts/Supplemental"), &mut faces);
    assert!(!faces.is_empty());
    let mut failed: Vec<String> = run(&faces)
        .iter()
        .map(|line| {
            line.split_whitespace()
                .take_while(|w| !matches!(*w, "new" | "reconfigure"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    failed.sort();
    failed.dedup();
    let known_missing: Vec<_> = KNOWN_UNHINTABLE
        .iter()
        .filter(|k| !failed.iter().any(|f| f.ends_with(*k)))
        .collect();
    let unknown: Vec<_> = failed
        .iter()
        .filter(|f| !KNOWN_UNHINTABLE.iter().any(|k| f.ends_with(k)))
        .collect();
    assert!(
        known_missing.is_empty() && unknown.is_empty(),
        "unhintable set changed: newly failing {unknown:?}, no longer failing {known_missing:?}"
    );
}
