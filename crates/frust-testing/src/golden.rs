//! Golden-image load/compare/store.
//!
//! A promoted baseline lives at `testing/goldens/<class>/<name>.png` with its
//! [`GoldenMeta`] provenance record beside it at `testing/goldens/<class>/
//! <name>.json` (`docs/TESTING.md`'s Golden Classes/Golden Image Policy).
//! [`compare_golden`] is the one entry point:
//!
//! - **`UPDATE_GOLDENS=1`([`update_goldens_enabled`]) is the only path that
//!   writes a baseline.** A normal run never touches `testing/goldens/` —
//!   `docs/TESTING.md`'s Baseline Updates section is explicit that "normal
//!   test runs must never write expected files".
//! - A [`crate::case::CaseSpec`] marked [`crate::case::CaseSpec::no_ref`]
//!   has no baseline to compare against yet; its rendered output is written
//!   as a review artifact instead of being diffed.
//! - On a normal comparison failure, the expected/actual/triptych images and
//!   the [`crate::diff::DiffReport`] JSON are written under `target/
//!   frust-testing/<class>/<name>/` for CI/local inspection — never under
//!   `testing/goldens/`.
//!
//! [`compare_golden`] resolves both roots (the golden corpus and the
//! failure-artifact directory) from this crate's own build-time
//! `CARGO_MANIFEST_DIR`, honoring `CARGO_TARGET_DIR` for the latter the same
//! way `cargo` itself does. [`compare_golden_at`] takes both roots
//! explicitly instead, which is what this module's own tests use to keep a
//! scratch run out of the real repository tree.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::case::CaseSpec;
use crate::diff::{self, DiffReport};
use crate::meta::GoldenMeta;
use crate::render::{AlphaKind, RenderedImage};

/// The repository root, resolved from this crate's own manifest directory
/// (`crates/frust-testing`) so path resolution is working-directory
/// independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `testing/goldens/` at the repository root — the only directory
/// [`compare_golden`] ever writes a promoted baseline into, and only when
/// [`update_goldens_enabled`] is `true`.
pub fn goldens_root() -> PathBuf {
    workspace_root().join("testing").join("goldens")
}

/// The build's target directory: `CARGO_TARGET_DIR` when set (relative
/// values resolve against the workspace root, matching `cargo`'s own
/// resolution), else `<workspace_root>/target`.
fn target_dir() -> PathBuf {
    match std::env::var("CARGO_TARGET_DIR") {
        Ok(dir) if !dir.is_empty() => {
            let path = PathBuf::from(dir);
            if path.is_absolute() {
                path
            } else {
                workspace_root().join(path)
            }
        }
        _ => workspace_root().join("target"),
    }
}

/// `target/frust-testing/` (or under `CARGO_TARGET_DIR`) — where
/// [`compare_golden`] writes review/failure artifacts. Never inside
/// `testing/goldens/`.
pub fn artifacts_root() -> PathBuf {
    target_dir().join("frust-testing")
}

/// Whether `UPDATE_GOLDENS=1` is set in the environment — the only signal
/// [`compare_golden`] accepts to permit a baseline write.
pub fn update_goldens_enabled() -> bool {
    matches!(std::env::var("UPDATE_GOLDENS").as_deref(), Ok(v) if v == "1")
}

fn image_path(goldens_root: &Path, class: &str, name: &str) -> PathBuf {
    goldens_root.join(class).join(format!("{name}.png"))
}

fn meta_path(goldens_root: &Path, class: &str, name: &str) -> PathBuf {
    goldens_root.join(class).join(format!("{name}.json"))
}

fn case_artifact_dir(artifacts_root: &Path, class: &str, name: &str) -> PathBuf {
    artifacts_root.join(class).join(name)
}

/// Both roots [`compare_golden_at`] resolves paths under, bundled so the
/// function takes one path-pair parameter instead of two separate `&Path`
/// ones.
pub struct GoldenRoots<'a> {
    /// `testing/goldens/` — the golden corpus root.
    pub goldens: &'a Path,
    /// `target/frust-testing/` (or under `CARGO_TARGET_DIR`) — the
    /// review/failure artifact root.
    pub artifacts: &'a Path,
}

/// The result of [`compare_golden`]/[`compare_golden_at`].
#[derive(Debug)]
pub struct GoldenOutcome {
    /// `true` when the case needs no further attention: an exact/tolerant
    /// match, a freshly written baseline, or a `no_ref` case recorded
    /// without a baseline to compare against.
    pub passed: bool,
    /// `true` when this call wrote a fresh baseline to `testing/goldens/`
    /// (only possible when `update` was `true`).
    pub updated: bool,
    /// The comparison report, when one was produced — `None` for a baseline
    /// write or a `no_ref` case with nothing to diff against.
    pub report: Option<DiffReport>,
    /// The directory review/failure artifacts were written to, when any
    /// were.
    pub artifact_dir: Option<PathBuf>,
}

/// Loads, compares, and (only under `UPDATE_GOLDENS=1`) stores the golden
/// baseline for `case` in golden class `class`, using this crate's own
/// resolved `testing/goldens/`/`target/frust-testing/` roots.
///
/// `meta` is the provenance record stored alongside a baseline write or a
/// `no_ref` review artifact — see [`GoldenMeta`]. `eroded_interior` is
/// forwarded to [`diff::diff_images`].
pub fn compare_golden(
    class: &str,
    case: &CaseSpec,
    actual: &RenderedImage,
    meta: &GoldenMeta,
    eroded_interior: bool,
    update: bool,
) -> Result<GoldenOutcome> {
    compare_golden_at(
        &GoldenRoots {
            goldens: &goldens_root(),
            artifacts: &artifacts_root(),
        },
        class,
        case,
        actual,
        meta,
        eroded_interior,
        update,
    )
}

/// [`compare_golden`] with both roots passed explicitly rather than resolved
/// from this crate's own build-time location — the seam this module's own
/// tests use to run against a scratch directory instead of the real
/// repository tree.
pub fn compare_golden_at(
    roots: &GoldenRoots<'_>,
    class: &str,
    case: &CaseSpec,
    actual: &RenderedImage,
    meta: &GoldenMeta,
    eroded_interior: bool,
    update: bool,
) -> Result<GoldenOutcome> {
    let goldens_root = roots.goldens;
    let artifacts_root = roots.artifacts;
    if actual.alpha != AlphaKind::Straight {
        bail!(
            "golden storage requires straight alpha; case `{}` produced a premultiplied \
             `RenderedImage` — convert before comparing/storing it as a golden",
            case.name
        );
    }

    let actual_image = RgbaImage::from_raw(actual.width, actual.height, actual.rgba8.clone())
        .with_context(|| {
            format!(
                "case `{}`: RenderedImage.rgba8 ({} bytes) doesn't match {}x{}x4",
                case.name,
                actual.rgba8.len(),
                actual.width,
                actual.height
            )
        })?;

    let image_path = image_path(goldens_root, class, case.name);
    let meta_path = meta_path(goldens_root, class, case.name);

    if update {
        write_baseline(&image_path, &meta_path, &actual_image, meta)?;
        return Ok(GoldenOutcome {
            passed: true,
            updated: true,
            report: None,
            artifact_dir: None,
        });
    }

    if case.no_ref {
        let dir = case_artifact_dir(artifacts_root, class, case.name);
        write_no_ref_artifact(&dir, &actual_image, meta)?;
        return Ok(GoldenOutcome {
            passed: true,
            updated: false,
            report: None,
            artifact_dir: Some(dir),
        });
    }

    if !image_path.is_file() {
        bail!(
            "no golden baseline at {} for case `{}` — run with UPDATE_GOLDENS=1 to create it, \
             or mark the case `no_ref` until a baseline is reviewed",
            image_path.display(),
            case.name
        );
    }

    let expected_image = image::open(&image_path)
        .with_context(|| format!("loading golden baseline {}", image_path.display()))?
        .to_rgba8();

    let outcome = diff::diff_images(
        &expected_image,
        &actual_image,
        case.tolerance,
        eroded_interior,
    );

    if outcome.passed {
        return Ok(GoldenOutcome {
            passed: true,
            updated: false,
            report: Some(outcome.report),
            artifact_dir: None,
        });
    }

    let dir = case_artifact_dir(artifacts_root, class, case.name);
    write_failure_artifacts(&dir, &expected_image, &actual_image, &outcome, meta)?;

    Ok(GoldenOutcome {
        passed: false,
        updated: false,
        report: Some(outcome.report),
        artifact_dir: Some(dir),
    })
}

fn write_baseline(
    image_path: &Path,
    meta_path: &Path,
    image: &RgbaImage,
    meta: &GoldenMeta,
) -> Result<()> {
    if let Some(parent) = image_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating golden directory {}", parent.display()))?;
    }
    image
        .save(image_path)
        .with_context(|| format!("writing golden baseline {}", image_path.display()))?;
    write_json(meta_path, meta)
}

fn write_no_ref_artifact(dir: &Path, actual: &RgbaImage, meta: &GoldenMeta) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating artifact dir {}", dir.display()))?;
    actual
        .save(dir.join("actual.png"))
        .with_context(|| format!("writing {}/actual.png", dir.display()))?;
    write_json(&dir.join("meta.json"), meta)
}

fn write_failure_artifacts(
    dir: &Path,
    expected: &RgbaImage,
    actual: &RgbaImage,
    outcome: &diff::DiffOutcome,
    meta: &GoldenMeta,
) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating artifact dir {}", dir.display()))?;
    expected
        .save(dir.join("expected.png"))
        .with_context(|| format!("writing {}/expected.png", dir.display()))?;
    actual
        .save(dir.join("actual.png"))
        .with_context(|| format!("writing {}/actual.png", dir.display()))?;
    outcome
        .triptych
        .save(dir.join("triptych.png"))
        .with_context(|| format!("writing {}/triptych.png", dir.display()))?;
    write_json(&dir.join("diff.json"), &outcome.report)?;
    write_json(&dir.join("meta.json"), meta)
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)
        .with_context(|| format!("serializing {}", path.display()))?;
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::Tolerance;
    use crate::render::BackendMeta;

    fn backend_meta() -> BackendMeta {
        BackendMeta {
            backend: "cpu".into(),
            adapter: "vello_cpu".into(),
            driver: "0.2.0".into(),
            device_kind: "software".into(),
        }
    }

    fn golden_meta(case: &str) -> GoldenMeta {
        GoldenMeta::new(
            &backend_meta(),
            "linux",
            "abc1234",
            case,
            Tolerance::exact(),
            "2026-08-29T00:00:00Z",
        )
    }

    fn rendered(width: u32, height: u32, color: [u8; 4]) -> RenderedImage {
        RenderedImage {
            width,
            height,
            rgba8: color.repeat((width * height) as usize),
            alpha: AlphaKind::Straight,
            meta: backend_meta(),
        }
    }

    /// A fresh scratch pair of (goldens_root, artifacts_root) directories
    /// under `std::env::temp_dir()` — never under the repository tree, so a
    /// test run can never write into the real `testing/goldens/` corpus.
    struct Scratch {
        goldens: PathBuf,
        artifacts: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "frust-testing-golden-selftest-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock")
                    .as_nanos()
            ));
            let goldens = base.join("goldens");
            let artifacts = base.join("artifacts");
            fs::create_dir_all(&goldens).expect("create scratch goldens dir");
            fs::create_dir_all(&artifacts).expect("create scratch artifacts dir");
            Self { goldens, artifacts }
        }

        fn roots(&self) -> GoldenRoots<'_> {
            GoldenRoots {
                goldens: &self.goldens,
                artifacts: &self.artifacts,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Some(parent) = self.goldens.parent() {
                let _ = fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn update_goldens_writes_baseline_and_meta_only_when_enabled() {
        let scratch = Scratch::new("update");
        let case = CaseSpec::new("solid_fill").with_size(2, 2);
        let actual = rendered(2, 2, [10, 20, 30, 255]);
        let meta = golden_meta("solid_fill");

        let outcome =
            compare_golden_at(&scratch.roots(), "cpu", &case, &actual, &meta, false, true)
                .expect("update run");

        assert!(outcome.passed);
        assert!(outcome.updated);
        assert!(outcome.report.is_none());

        let image_path = image_path(&scratch.goldens, "cpu", "solid_fill");
        let meta_path = meta_path(&scratch.goldens, "cpu", "solid_fill");
        assert!(image_path.is_file());
        assert!(meta_path.is_file());

        let stored =
            serde_json::from_str::<GoldenMeta>(&fs::read_to_string(&meta_path).expect("read meta"))
                .expect("parse meta");
        assert_eq!(stored, meta);
    }

    #[test]
    fn a_normal_run_never_writes_the_golden_corpus() {
        let scratch = Scratch::new("no-write");
        let case = CaseSpec::new("solid_fill").with_size(2, 2);
        let actual = rendered(2, 2, [10, 20, 30, 255]);
        let meta = golden_meta("solid_fill");

        // Seed a passing baseline first (via UPDATE_GOLDENS-equivalent),
        // then rerun without `update` and assert the file's mtime/content
        // are untouched by re-writing it and diffing.
        compare_golden_at(&scratch.roots(), "cpu", &case, &actual, &meta, false, true)
            .expect("seed baseline");

        let image_path = image_path(&scratch.goldens, "cpu", "solid_fill");
        let before = fs::read(&image_path).expect("read seeded baseline");

        let outcome =
            compare_golden_at(&scratch.roots(), "cpu", &case, &actual, &meta, false, false)
                .expect("normal comparison run");

        assert!(outcome.passed);
        assert!(!outcome.updated);
        assert!(outcome.report.is_some());
        let after = fs::read(&image_path).expect("read baseline after normal run");
        assert_eq!(
            before, after,
            "a normal run must never rewrite the baseline"
        );
    }

    #[test]
    fn a_missing_baseline_without_no_ref_is_an_error() {
        let scratch = Scratch::new("missing");
        let case = CaseSpec::new("never_promoted").with_size(2, 2);
        let actual = rendered(2, 2, [1, 2, 3, 255]);
        let meta = golden_meta("never_promoted");

        let result =
            compare_golden_at(&scratch.roots(), "cpu", &case, &actual, &meta, false, false);
        assert!(result.is_err());
    }

    #[test]
    fn a_no_ref_case_passes_and_records_an_artifact_without_comparing() {
        let scratch = Scratch::new("no-ref");
        let case = CaseSpec::new("awaiting_review")
            .with_size(2, 2)
            .with_no_ref(true);
        let actual = rendered(2, 2, [4, 5, 6, 255]);
        let meta = golden_meta("awaiting_review");

        let outcome =
            compare_golden_at(&scratch.roots(), "cpu", &case, &actual, &meta, false, false)
                .expect("no_ref run");

        assert!(outcome.passed);
        assert!(!outcome.updated);
        assert!(outcome.report.is_none());
        let dir = outcome.artifact_dir.expect("artifact dir recorded");
        assert!(dir.join("actual.png").is_file());
        assert!(dir.join("meta.json").is_file());
        assert!(!image_path(&scratch.goldens, "cpu", "awaiting_review").is_file());
    }

    #[test]
    fn a_mismatch_writes_expected_actual_triptych_and_diff_json() {
        let scratch = Scratch::new("mismatch");
        let case = CaseSpec::new("solid_fill")
            .with_size(2, 2)
            .with_tolerance(Tolerance::exact());
        let baseline = rendered(2, 2, [0, 0, 0, 255]);
        let meta = golden_meta("solid_fill");

        compare_golden_at(
            &scratch.roots(),
            "cpu",
            &case,
            &baseline,
            &meta,
            false,
            true,
        )
        .expect("seed baseline");

        let mismatched = rendered(2, 2, [255, 0, 0, 255]);
        let outcome = compare_golden_at(
            &scratch.roots(),
            "cpu",
            &case,
            &mismatched,
            &meta,
            false,
            false,
        )
        .expect("comparison run");

        assert!(!outcome.passed);
        let report = outcome.report.expect("report on mismatch");
        assert_eq!(report.pixel_count, 4);

        let dir = outcome.artifact_dir.expect("artifact dir on mismatch");
        assert!(dir.join("expected.png").is_file());
        assert!(dir.join("actual.png").is_file());
        assert!(dir.join("triptych.png").is_file());
        assert!(dir.join("diff.json").is_file());
        assert!(dir.join("meta.json").is_file());

        let stored_report: DiffReport = serde_json::from_str(
            &fs::read_to_string(dir.join("diff.json")).expect("read diff.json"),
        )
        .expect("parse diff.json");
        assert_eq!(stored_report, report);
    }

    #[test]
    fn premultiplied_alpha_is_refused() {
        let scratch = Scratch::new("premultiplied");
        let case = CaseSpec::new("solid_fill").with_size(1, 1);
        let mut actual = rendered(1, 1, [1, 2, 3, 4]);
        actual.alpha = AlphaKind::Premultiplied;
        let meta = golden_meta("solid_fill");

        let result =
            compare_golden_at(&scratch.roots(), "cpu", &case, &actual, &meta, false, false);
        assert!(result.is_err());
    }
}
