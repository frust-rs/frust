//! The unit corpus's golden gate: every [`frust_testing::corpus::unit`] case
//! rendered by both arms of the oracle pair and compared against its own
//! golden class.
//!
//! ```text
//! # the CPU arm — no GPU, no environment, part of the ordinary gate:
//! cargo test -p frust-testing --test goldens
//!
//! # the classic (GPU) arm, on the pinned runner:
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test goldens -- --ignored --nocapture
//! ```
//!
//! # What each arm gates
//!
//! The two arms are NOT held to the same bar, and deliberately so:
//!
//! - **`cpu/` is baseline-REQUIRED.** `vello_cpu` 0.2.0 rasterizes
//!   identically on any host of the same target (pinned SIMD level, zero
//!   worker threads — see `oracle_cpu`'s Determinism section), so a missing
//!   or mismatched baseline is a real failure on any machine. These are the
//!   PNGs this corpus commits.
//! - **A GPU class is baseline-OPTIONAL until someone promotes it.** A
//!   promoted GPU baseline is adapter- and driver-specific
//!   (`docs/TESTING.md`'s Golden Classes), so a class directory that does not
//!   exist yet means "not reviewed on this runner", not "regression": the
//!   case still renders, still runs its probes, and records its output as a
//!   review artifact under `target/frust-testing/`. The moment a baseline is
//!   committed under `testing/goldens/<class>/`, the same run starts
//!   comparing against it with no code change.
//!
//! What makes the GPU arm a real gate in either state is [`Probe`]s: the
//! absolute pixel arithmetic the two promoted cases carry (see
//! `corpus::unit`) is asserted on every backend, baseline or no baseline.
//!
//! # Promotion
//!
//! `UPDATE_GOLDENS=1` is the only path that writes a baseline
//! (`frust_testing::golden`), and this harness narrows it twice more:
//!
//! - a case whose probes FAIL is never promoted — a baseline is only ever
//!   written from a frame that already satisfies its absolute assertions;
//! - a GPU arm on an adapter with no reviewed golden class
//!   ([`frust_testing::UNCLASSIFIED_CLASS`]) refuses to promote at all,
//!   rather than inventing a directory name for an unreviewed machine.

use std::time::{SystemTime, UNIX_EPOCH};

use frust_testing::case::CaseSpec;
use frust_testing::corpus::{CorpusCase, render_case, unit_cases};
use frust_testing::golden::{compare_golden, goldens_root, update_goldens_enabled};
use frust_testing::meta::GoldenMeta;
use frust_testing::oracle_classic::ClassicOracle;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::render::SceneRenderer;
use frust_testing::{ORACLE_ID, UNCLASSIFIED_CLASS};

/// The golden class the CPU oracle's baselines live in
/// (`testing/goldens/cpu/`).
///
/// Not the oracle's own [`ORACLE_ID`] (`vello-cpu-0.2`): `docs/TESTING.md`
/// names this class `cpu/`, and a class directory outlives the exact
/// rasterizer version behind it. The two are tied together by
/// [`the_cpu_class_is_backed_by_the_pinned_oracle`], so a version bump has to
/// notice this mapping instead of silently re-pointing the directory.
const CPU_CLASS: &str = "cpu";

/// Whether a run is allowed to write baselines, and whether a missing one is
/// a failure.
struct RunPolicy {
    /// The golden class directory the case's baseline is read from/written
    /// to.
    class: &'static str,
    /// When `true`, a case with no stored baseline FAILS. When `false`, it
    /// records a review artifact and passes (see the module docs).
    require_baseline: bool,
    /// When `true`, `UPDATE_GOLDENS=1` may promote from this run.
    promotable: bool,
}

/// The repository commit this run's baselines would be attributed to.
///
/// Shelled out to `git` rather than baked in at build time: a `env!`-style
/// capture would be frozen at the last recompilation, which is exactly when
/// it would be wrong (a rebuild-free re-run after a commit). Falls back to
/// `"unknown"` — a missing commit is worth recording as missing, not worth
/// failing a render over.
fn frust_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// The current UTC time as an RFC 3339 timestamp (`GoldenMeta::timestamp`'s
/// contract).
///
/// Hand-rolled from [`SystemTime`] rather than pulling a date crate into the
/// workspace for one field: the civil-date conversion below is Howard
/// Hinnant's `civil_from_days`, shifted so the era starts on 0000-03-01, and
/// this repository's Version-Pin Policy makes a new third-party pin the more
/// expensive of the two options. Always UTC, so no local-timezone state
/// leaks into a promoted baseline's provenance.
fn rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let time_of_day = secs % 86_400;
    let (hour, minute, second) = (
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60,
    );

    // Days since 1970-01-01 -> civil date. 719_468 shifts the epoch onto
    // 0000-03-01, where a 400-year era's leap pattern is uniform.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    // `mp` counts months from March; fold it back onto a January start.
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The `GoldenMeta` provenance record stored beside a promoted baseline or a
/// review artifact.
fn golden_meta(renderer: &dyn SceneRenderer, spec: &CaseSpec) -> GoldenMeta {
    GoldenMeta::new(
        &renderer.meta(),
        std::env::consts::OS,
        frust_commit(),
        spec.name,
        spec.tolerance,
        rfc3339_now(),
    )
}

/// Whether a baseline PNG already exists for `case` in `class`.
fn baseline_exists(class: &str, case: &CorpusCase) -> bool {
    goldens_root()
        .join(class)
        .join(format!("{}.png", case.spec.name))
        .is_file()
}

/// Renders every unit case on `renderer`, checks its probes, compares (or
/// records) its baseline, and panics with EVERY failure at once.
///
/// Reporting the whole corpus rather than the first red case is deliberate:
/// a rasterizer change usually moves many cases, and a one-at-a-time gate
/// turns that into as many edit-run cycles as there are cases.
fn run_corpus(renderer: &mut dyn SceneRenderer, policy: &RunPolicy) {
    let update = update_goldens_enabled() && policy.promotable;
    if update_goldens_enabled() && !policy.promotable {
        println!(
            "goldens: UPDATE_GOLDENS=1 ignored — backend `{}` has no reviewed golden class",
            renderer.id()
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0_usize;
    let mut recorded = 0_usize;
    let mut promoted = 0_usize;
    let mut skipped = 0_usize;

    for case in unit_cases() {
        let Some(image) = render_case(renderer, &case)
            .unwrap_or_else(|err| panic!("case `{}` failed to render: {err:#}", case.spec.name))
        else {
            skipped += 1;
            println!(
                "goldens: `{}` skipped on `{}`",
                case.spec.name,
                renderer.id()
            );
            continue;
        };

        // Probes first, and a red probe stops promotion: a baseline is only
        // ever written from a frame that already satisfies the case's own
        // absolute assertions.
        let probe_failures = case.failed_probes(&image);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                failures.push(format!("[{}] probe: {message}", case.spec.name));
            }
            continue;
        }

        // A GPU class that nobody has promoted yet records instead of
        // comparing (see the module docs); the `cpu/` class never does.
        let mut spec = case.spec.clone();
        if !policy.require_baseline && !baseline_exists(policy.class, &case) {
            spec.no_ref = true;
        }

        let meta = golden_meta(renderer, &spec);
        let outcome = match compare_golden(
            policy.class,
            &spec,
            &image,
            &meta,
            case.eroded_interior,
            update,
        ) {
            Ok(outcome) => outcome,
            // A missing baseline, an unreadable PNG, a premultiplied frame:
            // recorded like any other case failure rather than aborting the
            // run, so one command reports the whole corpus (see this
            // function's docs).
            Err(err) => {
                failures.push(format!("[{}] {err:#}", case.spec.name));
                continue;
            }
        };

        if outcome.updated {
            promoted += 1;
        } else if spec.no_ref {
            recorded += 1;
        } else {
            compared += 1;
        }

        if !outcome.passed {
            let report = outcome
                .report
                .as_ref()
                .map(|r| {
                    format!(
                        "{} px differ ({:.4}%), max |delta| {:?}, bbox {:?}",
                        r.pixel_count, r.mismatched_percent, r.max_difference, r.bounding_box
                    )
                })
                .unwrap_or_else(|| "no report".to_string());
            let artifacts = outcome
                .artifact_dir
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "<none>".to_string());
            failures.push(format!(
                "[{}] golden mismatch under tolerance {:?}: {report}; artifacts in {artifacts}",
                case.spec.name, case.spec.tolerance
            ));
        }
    }

    println!(
        "goldens: class `{}` — {compared} compared, {recorded} recorded (no baseline yet), \
         {promoted} promoted, {skipped} skipped",
        policy.class
    );

    assert!(
        failures.is_empty(),
        "{} corpus failure(s) in golden class `{}`:\n{}",
        failures.len(),
        policy.class,
        failures.join("\n")
    );
}

/// The gate that runs everywhere: the whole unit corpus against the
/// committed `cpu/` baselines, with no GPU involved.
#[test]
fn cpu_corpus_matches_its_goldens() {
    let mut oracle = CpuOracle::new();
    run_corpus(
        &mut oracle,
        &RunPolicy {
            class: CPU_CLASS,
            require_baseline: true,
            promotable: true,
        },
    );
}

/// Every case the CPU arm actually compares must have rendered with NO
/// fidelity gap.
///
/// `CpuOracle` reports its downgrades (a shader quad with no GPU, an image
/// brush handed to a fill, an unparseable font, an unrenderable image)
/// through `SkipReport` instead of failing. A promoted `cpu/` baseline of a
/// downgraded frame would be a baseline of the downgrade, so this asserts the
/// report is empty for every case that reaches the comparator —
/// `unit-shader-quad`, the one case with a genuine CPU gap, is excluded by
/// its own skip set and never renders here at all.
#[test]
fn the_cpu_oracle_reports_no_fidelity_gap_on_any_compared_case() {
    let mut oracle = CpuOracle::new();
    for case in unit_cases() {
        let rendered = render_case(&mut oracle, &case)
            .unwrap_or_else(|err| panic!("case `{}` failed to render: {err:#}", case.spec.name));
        if rendered.is_none() {
            continue;
        }
        let skips = oracle.skips();
        assert!(
            skips.is_empty(),
            "case `{}` rendered with a CPU fidelity gap ({skips:?}) — its baseline would \
             record the downgrade, not the command",
            case.spec.name
        );
    }
}

/// The `cpu/` class directory is the pinned oracle's, and stays that way.
///
/// A tripwire on the class-name mapping rather than an assertion about
/// pixels: if the oracle's rasterizer identity ever changes, this is the test
/// that asks whether `testing/goldens/cpu/` still means what its committed
/// PNGs meant.
#[test]
fn the_cpu_class_is_backed_by_the_pinned_oracle() {
    assert_eq!(
        CpuOracle::new().id(),
        ORACLE_ID,
        "the `cpu/` golden class is defined as `{ORACLE_ID}`'s output"
    );
}

/// The classic (GPU) arm, on real hardware.
///
/// `#[ignore]`d like every other GPU test in this workspace: CI sandboxes
/// without a GPU skip it, and a local pass on a deliberately selected adapter
/// is the gate. Name the adapter on a multi-GPU host — this machine
/// enumerates both an NVIDIA T400 and an Intel iGPU, and
/// `FRUST_GOLDEN_EXPECT_ADAPTER` turns "which one did I get?" into a refusal
/// rather than a footnote:
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test goldens -- --ignored --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test goldens -- --ignored`"]
fn classic_corpus_matches_its_goldens() {
    let mut oracle = ClassicOracle::new(frust_render::HeadlessOptions::default())
        .expect("failed to create the classic (headless GPU) oracle");
    println!("goldens: classic arm on {}", oracle.headless_meta());
    println!("goldens: golden class `{}`", oracle.id());

    let class = oracle.id();
    let classified = oracle.is_classified();
    if !classified {
        println!(
            "goldens: adapter has no reviewed golden class — rendering and probing every case, \
             comparing none (see this test file's module docs)"
        );
    }
    run_corpus(
        &mut oracle,
        &RunPolicy {
            class,
            // A GPU baseline is adapter/driver-specific and is promoted by a
            // reviewed, deliberate act; a missing one is never a failure
            // here.
            require_baseline: false,
            promotable: classified,
        },
    );
}

/// The unclassified fallback is never a directory anyone can promote into.
///
/// A cheap, GPU-free guard on the safety property the GPU test's
/// `promotable` flag depends on: even with `UPDATE_GOLDENS=1` set, an
/// unrecognized adapter must not create `testing/goldens/<something>/`.
#[test]
fn the_unclassified_class_is_not_a_committed_golden_directory() {
    assert!(
        !goldens_root().join(UNCLASSIFIED_CLASS).exists(),
        "`{UNCLASSIFIED_CLASS}` is the refuse-to-promote fallback — it must never become a \
         committed golden class directory"
    );
}

/// The provenance timestamp is the shape `GoldenMeta::timestamp` promises.
#[test]
fn rfc3339_now_has_the_shape_golden_meta_promises() {
    let stamp = rfc3339_now();
    assert_eq!(stamp.len(), 20, "{stamp}");
    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(&stamp[4..5], "-", "{stamp}");
    assert_eq!(&stamp[7..8], "-", "{stamp}");
    assert_eq!(&stamp[10..11], "T", "{stamp}");
    let year: i64 = stamp[..4].parse().expect("year");
    assert!(year >= 2026, "{stamp} predates this test being written");
}
