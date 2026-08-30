//! The engine's primary correctness gate: every corpus case this phase's
//! engine can draw, rendered by [`EngineOracle`] and held against two
//! references at once — `vello_cpu` 0.2.0 (hard) and the engine's own
//! committed golden class (hard once promoted) — plus a recorded, advisory
//! comparison against the vello-classic pipeline.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-testing --test engine_goldens -- --ignored --nocapture
//! ```
//!
//! The host-only tests in this file (scope coverage, class routing) run in the
//! ordinary gate with no GPU.
//!
//! # The two comparisons, and why they are held to different bars
//!
//! - **engine vs `vello_cpu` is near-exact and gating.** Both rasterize the
//!   same geometry with the same `vello_common` 0.2.0 strip generator at the
//!   same flattening tolerance; only the *fill* differs (a GPU strip pass
//!   against a CPU one). So the bar is the corpus's own tight default —
//!   channel 2, alpha 2, zero tolerated mismatched pixels — carried straight
//!   off each [`CaseSpec`], with a per-case escalation only through
//!   [`ESCALATIONS`] and only with a stated reason. It is made on the
//!   PREMULTIPLIED frames both arms natively produce rather than on their
//!   straightened forms — see [`render_raw`], where that is the difference
//!   between measuring the rasterizers and measuring an unpremultiply's own
//!   information loss.
//! - **engine vs classic is a wide, measured band and ADVISORY here.** The two
//!   are independent rasterizers, and the classic arm hands back STRAIGHT
//!   alpha, so that comparison necessarily goes through [`render_case`]'s
//!   conversion — the same footing `testing/goldens/CALIBRATION.md` measured
//!   its numbers on. That file measured
//!   how far they legitimately drift over this same corpus: mean absolute
//!   error <= 2.5 and <= 4.8% of pixels over channel 8, both on the 1-px eroded
//!   interior, with the per-channel maximum deliberately ungated (antialiasing
//!   conflation along a diagonal or curved edge exceeds what a 1-px erosion
//!   removes — see that file's Legitimate Disagreements). This phase RECORDS
//!   those numbers per case and never fails on them; the gate hardens once the
//!   engine covers the whole corpus.
//!
//! # Which cases are in scope
//!
//! The engine compiles axis-aligned rectangles, rounded rectangles, lines and
//! arbitrary filled/stroked paths, and resolves solid and gradient paints.
//! Clips, layers, glyph runs, images, blurs, shader quads and snapshot
//! brackets are recognised and skipped by the compiler — a frame draws less,
//! never wrong — so a case built out of them would compare an engine frame
//! that legitimately omits its subject. [`PHASE_CASES`] names the cases whose
//! subject the engine actually draws and [`DEFERRED_CASES`] every other one
//! with the phase it waits for;
//! [`every_corpus_case_is_either_in_scope_or_deferred`] fails if a corpus
//! addition appears in neither, so growth forces a decision instead of
//! silently widening or narrowing the gate.
//!
//! # Promotion
//!
//! `UPDATE_GOLDENS=1` is the only path that writes a baseline
//! (`frust_testing::golden`), narrowed twice more here, exactly as the other
//! golden gates narrow it: a case whose probes or cross-arm comparison FAIL is
//! never promoted, and an adapter with no reviewed engine class
//! ([`frust_testing::ENGINE_UNCLASSIFIED_CLASS`]) refuses to promote at all
//! rather than inventing a directory for an unreviewed machine.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use image::RgbaImage;

use frust_testing::case::{CaseSpec, Tolerance};
use frust_testing::corpus::{
    CorpusCase, adversarial_cases, render_case, straighten_alpha, unit_cases,
};
use frust_testing::diff::{DiffReport, diff_images};
use frust_testing::frame::foreign_font_runs;
use frust_testing::golden::{compare_golden, goldens_root, update_goldens_enabled};
use frust_testing::meta::GoldenMeta;
use frust_testing::oracle_classic::ClassicOracle;
use frust_testing::oracle_cpu::CpuOracle;
use frust_testing::oracle_engine::{EngineOracle, EngineOracleOptions};
use frust_testing::render::{RenderedImage, SceneRenderer};
use frust_testing::{ENGINE_UNCLASSIFIED_CLASS, engine_golden_class};

/// Every corpus case the engine draws in full this phase — rectangles,
/// rounded rectangles, lines, arbitrary filled/stroked/dashed paths, and the
/// empty scene whose whole subject is the clear pass.
const PHASE_CASES: &[&str] = &[
    "unit-fill-rect",
    "unit-rounded-rect",
    "unit-stroke-line",
    "unit-path-fill",
    "unit-path-stroke",
    "unit-path-dashed",
    "adv-degenerate-path",
    "adv-subpixel-rrect",
    "adv-empty-scene",
    "adv-1px-divider-1x",
    "adv-1px-divider-2x",
    "adv-1px-divider-2-75x",
];

/// Every corpus case NOT in [`PHASE_CASES`], with why it waits.
///
/// A case is deferred when its subject is a command the engine's compiler
/// recognises and skips (so the frame it produces is deliberately missing the
/// very thing the case exists to pin), or when it has no stable reference on
/// any backend at all. Neither is a defect to chase here: both are
/// "the engine does not draw this yet", and comparing anyway would gate this
/// phase on a later one's work.
const DEFERRED_CASES: &[(&str, &str)] = &[
    ("unit-glyph-run", "glyph runs arrive with the text phase"),
    ("adv-10k-glyphs", "glyph runs arrive with the text phase"),
    ("unit-clip-rect", "clips arrive with the clip phase"),
    ("unit-clip-rounded", "clips arrive with the clip phase"),
    ("unit-clip-balance", "clips arrive with the clip phase"),
    ("adv-clip-nest-8", "clips arrive with the clip phase"),
    ("unit-layer-alpha", "layers arrive with the layer phase"),
    ("unit-layer-balance", "layers arrive with the layer phase"),
    (
        "unit-clear-rect",
        "the hole punch is a layer-group contract; layers arrive with the layer phase",
    ),
    (
        "unit-snapshot-bracket",
        "snapshot brackets lower to layers; both arrive with the layer phase",
    ),
    (
        "unit-snapshot-balance",
        "snapshot brackets lower to layers; both arrive with the layer phase",
    ),
    (
        "adv-destout-in-layer",
        "a hole punch inside a layer group; layers arrive with the layer phase",
    ),
    (
        "adv-snapshot-scale-alpha",
        "snapshot brackets lower to layers; both arrive with the layer phase",
    ),
    (
        "adv-unbalanced-pops",
        "unwinds clip, layer AND snapshot stacks; all three arrive later",
    ),
    (
        "adv-unbalanced-pop-in-snapshot",
        "`no_ref`, and an unbalanced pop inside a translucent snapshot bracket — snapshot \
         brackets lower to layers, which arrive with the layer phase",
    ),
    ("unit-image", "images need the atlas; they arrive with it"),
    (
        "adv-huge-image",
        "images need the atlas; they arrive with it",
    ),
    ("unit-blur-rrect", "blur arrives with the filter phase"),
    (
        "unit-shader-quad",
        "a shader pre-pass the engine does not own; also skipped on the CPU arm, so there \
         would be no reference either way",
    ),
    (
        "adv-nan-transform",
        "`no_ref`: deliberately malformed transform components, no stable reference on any \
         backend — and a non-finite transform is a refusal on the engine's frame path",
    ),
    (
        "adv-5k-layers",
        "`no_ref`: a depth-only memory-budget probe over 5,000 nested layers, which the \
         engine skips until the layer phase",
    ),
];

/// Per-case escalations off the corpus's tight default, each with the reason
/// it is not the default's fault.
///
/// Empty by design. `docs/TESTING.md`'s Comparison section puts thresholds on
/// "the golden class or named test, not an ad hoc retry path", and nothing in
/// this file re-runs a comparison at a looser threshold: an escalation is a
/// reviewed row here or it does not exist. It lives beside the harness rather
/// than on the shared [`CaseSpec`] because it is specific to the ENGINE-vs-CPU
/// pairing — the same case's `cpu/` and classic comparisons are unaffected by
/// anything written here.
const ESCALATIONS: &[(&str, Tolerance, &str)] = &[];

/// The fixed threshold the engine-vs-classic band is measured under, so
/// "percent of pixels over 8" means the same thing for every case and the same
/// thing `CALIBRATION.md` measured.
const MEASURE_TOLERANCE: Tolerance = Tolerance {
    channel: 8,
    alpha: 8,
    diff_pixels: 0,
};

/// `CALIBRATION.md`'s measured whole-corpus p95 mean absolute error between
/// the classic pipeline and `vello_cpu`, rounded up — the mean half of the
/// advisory engine-vs-classic band.
const CLASSIC_MEAN_BUDGET: f64 = 2.5;

/// `CALIBRATION.md`'s measured whole-corpus p95 of "% pixels over channel 8"
/// on the eroded interior, rounded up — the other half of that band. The
/// per-channel maximum has no budget on purpose: it is ungateable, since edge
/// conflation on a diagonal or curved boundary survives the 1-px erosion.
const CLASSIC_PCT_OVER_8_BUDGET: f64 = 4.8;

/// Serializes every test in this binary that creates a GPU device.
///
/// Two ignored GPU tests running concurrently have been observed to hang at
/// process teardown roughly one run in five: each holds its own device and
/// pipeline-warm-up state, and tearing both down at once races inside the
/// driver. Serializing costs nothing here (the whole scoped corpus is a
/// handful of 64x64 frames) and keeps a teardown race from being read as a
/// rendering fault. Poison is ignored deliberately: one test's failure must
/// not cascade into every sibling.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Renders `case` on `renderer` WITHOUT the straight-alpha conversion
/// [`render_case`] applies, or `None` when the case skips this backend.
///
/// The engine-vs-CPU comparison is made on these bytes, in the premultiplied
/// space both arms natively produce, and that is load-bearing rather than a
/// shortcut. Un-premultiplying divides each colour channel by the pixel's own
/// alpha, so at the low coverage along a hairline's end cap or a dash boundary
/// (alpha 1-11 of 255) it multiplies a difference by up to 255: two frames one
/// 8-bit level apart in the space they were computed in come out as `[0, 255,
/// 0, 1]` against `[0, 0, 0, 1]` once straightened. Comparing before that
/// conversion measures the rasterizers; comparing after it measures the
/// conversion's own documented information loss
/// ([`straighten_alpha`]'s docs), which no tolerance can distinguish from a
/// real regression.
///
/// The golden path still goes through [`straighten_alpha`]: a stored PNG is
/// straight alpha, and every arm's baseline has to be stored the same way to
/// be comparable at all.
fn render_raw(renderer: &mut dyn SceneRenderer, case: &CorpusCase) -> Option<RenderedImage> {
    if case.spec.skip.contains(renderer.id()) {
        return None;
    }
    let image = renderer
        .render(&case.scene(), &case.render_spec())
        .unwrap_or_else(|err| {
            panic!(
                "case `{}` failed to render on `{}`: {err:#}",
                case.spec.name,
                renderer.id()
            )
        });
    Some(image)
}

/// Every corpus case this phase compares, in corpus order.
fn scoped_cases() -> Vec<CorpusCase> {
    unit_cases()
        .into_iter()
        .chain(adversarial_cases())
        .filter(|case| PHASE_CASES.contains(&case.spec.name))
        .collect()
}

/// The tolerance `case` is compared against the CPU arm under: its own tight
/// default unless [`ESCALATIONS`] names it.
fn engine_tolerance(name: &str) -> (Tolerance, Option<&'static str>) {
    ESCALATIONS
        .iter()
        .find(|(case, _, _)| *case == name)
        .map_or((Tolerance::new(), None), |(_, tolerance, why)| {
            (*tolerance, Some(*why))
        })
}

/// The repository commit this run's baselines would be attributed to — see
/// `tests/goldens.rs`'s identical helper for why this shells out rather than
/// baking the SHA in at build time.
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

/// The current UTC time as an RFC 3339 timestamp — see `tests/goldens.rs`'s
/// identical helper for the civil-date conversion this hand-rolls rather than
/// pulling in a date crate.
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

/// Converts a straight-alpha [`RenderedImage`] into the [`RgbaImage`]
/// [`diff_images`] compares — the same conversion `golden.rs` performs before
/// every comparison.
fn to_rgba_image(image: &RenderedImage) -> RgbaImage {
    RgbaImage::from_raw(image.width, image.height, image.rgba8.clone()).unwrap_or_else(|| {
        panic!(
            "rendered {}x{} frame ({} bytes) does not match width*height*4",
            image.width,
            image.height,
            image.rgba8.len()
        )
    })
}

/// A one-line summary of a comparison, for both the failure message and the
/// recorded run log.
fn summarize(report: &DiffReport) -> String {
    format!(
        "{} px differ ({:.4}%), max |delta| {:?}, mean {:?}, bbox {:?}",
        report.pixel_count,
        report.mismatched_percent,
        report.max_difference,
        report.mean_abs_error.map(|v| (v * 1000.0).round() / 1000.0),
        report.bounding_box
    )
}

/// The first few differing pixels, expected against actual.
///
/// A cross-arm failure between two rasterizers sharing one geometry core is
/// almost always a handful of pixels with a specific shape (one end of a
/// hairline, one edge of a dash), and the counts alone do not say which — so
/// the failure message carries the actual bytes rather than only how many of
/// them there were.
fn first_pixels(report: &DiffReport) -> String {
    const SHOWN: usize = 6;
    let listed: Vec<String> = report
        .pixels
        .iter()
        .take(SHOWN)
        .map(|pixel| {
            format!(
                "({}, {}) cpu {:?} vs engine {:?}",
                pixel.x, pixel.y, pixel.expected, pixel.actual
            )
        })
        .collect();
    format!(
        "first {} of {}: {}",
        listed.len(),
        report.pixel_count,
        listed.join("; ")
    )
}

/// Renders every scoped case on the engine, compares it against the CPU arm at
/// the P1 bar and against the engine's own golden class, and reports EVERY
/// failure at once — the same one-run-reports-everything shape the other
/// golden gates use, so a rasterizer change surfaces every affected case in
/// one command.
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test engine_goldens -- --ignored --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test engine_goldens -- --ignored`"]
fn engine_corpus_matches_vello_cpu_and_its_goldens() {
    let _serialized = render_lock();
    let mut engine = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    let mut cpu = CpuOracle::new();

    let class = engine.id();
    let classified = engine.is_classified();
    println!("engine goldens: engine arm on {}", engine.adapter_meta());
    println!("engine goldens: golden class `{class}`");
    println!("engine goldens: cpu arm `{}`", cpu.id());
    if !classified {
        println!(
            "engine goldens: adapter has no reviewed engine golden class — comparing against \
             the CPU arm and probing every case, promoting nothing"
        );
    }

    let update = update_goldens_enabled() && classified;
    if update_goldens_enabled() && !classified {
        println!(
            "engine goldens: UPDATE_GOLDENS=1 ignored — backend `{class}` has no reviewed \
             golden class"
        );
    }

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0_usize;
    let mut recorded = 0_usize;
    let mut promoted = 0_usize;

    for case in scoped_cases() {
        let name = case.spec.name;

        // Font determinism first, before this case's frame is even rendered —
        // the same ordering every other corpus gate uses. No scoped case
        // carries a glyph run today; the check is what keeps that true.
        let foreign = foreign_font_runs(&case.scene());
        if !foreign.is_empty() {
            for message in foreign {
                failures.push(format!("[{name}] font: {message}"));
            }
            continue;
        }

        let Some(rendered) = render_raw(&mut engine, &case) else {
            failures.push(format!(
                "[{name}] the case skips backend `{class}` — a scoped case must be one the \
                 engine renders"
            ));
            continue;
        };
        let Some(reference) = render_raw(&mut cpu, &case) else {
            failures.push(format!(
                "[{name}] the case skips the CPU arm — a scoped case needs a reference"
            ));
            continue;
        };
        if rendered.alpha != reference.alpha {
            failures.push(format!(
                "[{name}] the two arms report different alpha conventions ({:?} vs {:?}) — a \
                 cross-arm diff would be comparing two different spaces",
                reference.alpha, rendered.alpha
            ));
            continue;
        }
        // Straight alpha for everything downstream of the cross-arm diff: the
        // probes were stated against straight frames and a golden PNG is
        // straight (see `render_raw`).
        let image = straighten_alpha(&rendered);

        // Probes first, and a red probe stops promotion: a baseline is only
        // ever written from a frame that already satisfies the case's own
        // absolute assertions.
        let probe_failures = case.failed_probes(&image);
        if !probe_failures.is_empty() {
            for message in probe_failures {
                failures.push(format!("[{name}] probe: {message}"));
            }
            continue;
        }

        // The primary comparison: the two arms share one geometry core, so
        // this is the assertion that the GPU fill agrees with the CPU one —
        // made on the premultiplied frames both produced (see `render_raw`).
        let (tolerance, escalation) = engine_tolerance(name);
        let outcome = diff_images(
            &to_rgba_image(&reference),
            &to_rgba_image(&rendered),
            tolerance,
            case.eroded_interior,
        );
        println!(
            "engine goldens: [{name}] vs cpu under {tolerance:?}{} — {}",
            escalation
                .map(|why| format!(" (escalated: {why})"))
                .unwrap_or_default(),
            summarize(&outcome.report)
        );
        if !outcome.passed {
            failures.push(format!(
                "[{name}] engine and `{}` disagree beyond {tolerance:?}: {}; {}",
                cpu.id(),
                summarize(&outcome.report),
                first_pixels(&outcome.report)
            ));
            continue;
        }

        // A class nobody has promoted yet records instead of comparing, the
        // same way the classic arm's own gate treats an unpromoted class.
        let mut spec = case.spec.clone();
        if !baseline_exists(class, &case) {
            spec.no_ref = true;
        }

        let meta = golden_meta(&engine, &spec);
        let golden = match compare_golden(class, &spec, &image, &meta, case.eroded_interior, update)
        {
            Ok(golden) => golden,
            Err(err) => {
                failures.push(format!("[{name}] {err:#}"));
                continue;
            }
        };

        if golden.updated {
            promoted += 1;
        } else if spec.no_ref {
            recorded += 1;
        } else {
            compared += 1;
        }

        if !golden.passed {
            let report = golden
                .report
                .as_ref()
                .map(summarize)
                .unwrap_or_else(|| "no report".to_string());
            let artifacts = golden
                .artifact_dir
                .as_ref()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| "<none>".to_string());
            failures.push(format!(
                "[{name}] golden mismatch under tolerance {:?}: {report}; artifacts in {artifacts}",
                spec.tolerance
            ));
        }
    }

    println!(
        "engine goldens: class `{class}` — {compared} compared, {recorded} recorded (no \
         baseline yet), {promoted} promoted, out of {} scoped case(s)",
        PHASE_CASES.len()
    );

    assert!(
        failures.is_empty(),
        "{} engine corpus failure(s) in golden class `{class}`:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Records how far the engine sits from the vello-classic pipeline over the
/// same scoped cases, against `CALIBRATION.md`'s measured band.
///
/// ADVISORY this phase: every number is printed, an over-band case is called
/// out, and nothing here fails the build. The two are independent
/// rasterizers — the band exists to make a *drift* visible while the engine is
/// still growing its command coverage, and hardens into a gate once it covers
/// the whole corpus.
///
/// ```text
/// WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
///   cargo test -p frust-testing --test engine_goldens -- --ignored --nocapture
/// ```
#[test]
#[ignore = "requires a GPU; run locally with `cargo test -p frust-testing --test engine_goldens -- --ignored`"]
fn engine_vs_classic_divergence_is_recorded_against_the_calibrated_band() {
    let _serialized = render_lock();
    let mut engine = EngineOracle::new(&EngineOracleOptions::default())
        .expect("failed to create the engine oracle");
    let mut classic = ClassicOracle::new(frust_render::HeadlessOptions::default())
        .expect("failed to create the classic (headless GPU) oracle");

    println!("engine vs classic: engine arm on {}", engine.adapter_meta());
    println!(
        "engine vs classic: classic arm on {}",
        classic.headless_meta()
    );
    println!(
        "engine vs classic: measured under {MEASURE_TOLERANCE:?}, advisory band mean <= \
         {CLASSIC_MEAN_BUDGET}, % over 8 <= {CLASSIC_PCT_OVER_8_BUDGET}% (testing/goldens/\
         CALIBRATION.md; per-channel max deliberately ungated)"
    );
    println!(
        "{:<24} {:>8} {:>10} {:>10}",
        "case", "mean", "% over 8", "chan max"
    );

    let mut over_band: Vec<String> = Vec::new();
    let mut measured = 0_usize;

    for case in scoped_cases() {
        let name = case.spec.name;
        let (Some(engine_image), Some(classic_image)) = (
            render_case(&mut engine, &case)
                .unwrap_or_else(|err| panic!("case `{name}` failed on the engine: {err:#}")),
            render_case(&mut classic, &case)
                .unwrap_or_else(|err| panic!("case `{name}` failed on classic: {err:#}")),
        ) else {
            println!("{name:<24} skipped on at least one arm — not measured");
            continue;
        };

        let outcome = diff_images(
            &to_rgba_image(&classic_image),
            &to_rgba_image(&engine_image),
            MEASURE_TOLERANCE,
            case.eroded_interior,
        );
        let report = outcome.report;
        // The eroded-interior per-channel maximum, printed for the record and
        // never gated: `DiffReport::max_difference` is a whole-image statistic
        // regardless of erosion, so it is the recorded pixels — the ones that
        // survived erosion AND exceeded the measurement tolerance — that say
        // anything about the interior.
        let channel_max = report
            .pixels
            .iter()
            .flat_map(|pixel| pixel.difference)
            .map(|delta| u8::try_from(delta.unsigned_abs()).unwrap_or(u8::MAX))
            .max()
            .unwrap_or(0);
        let mean = report.mean_abs_error.iter().sum::<f64>() / report.mean_abs_error.len() as f64;

        println!(
            "{name:<24} {mean:>8.3} {:>9.4}% {channel_max:>10}",
            report.mismatched_percent
        );
        measured += 1;

        if mean > CLASSIC_MEAN_BUDGET || report.mismatched_percent > CLASSIC_PCT_OVER_8_BUDGET {
            over_band.push(format!(
                "{name}: mean {mean:.3} (band {CLASSIC_MEAN_BUDGET}), % over 8 {:.4}% (band \
                 {CLASSIC_PCT_OVER_8_BUDGET}%)",
                report.mismatched_percent
            ));
        }
    }

    println!("engine vs classic: {measured} case(s) measured");
    if over_band.is_empty() {
        println!("engine vs classic: every measured case sits inside the calibrated band");
    } else {
        println!(
            "engine vs classic: {} case(s) OUTSIDE the calibrated band (advisory this phase, \
             recorded not failed):\n{}",
            over_band.len(),
            over_band.join("\n")
        );
    }
}

/// Every corpus case is either in scope for this phase or deferred with a
/// reason — a GPU-free tripwire, so a corpus addition is caught on any machine
/// rather than only on the pinned runner.
#[test]
fn every_corpus_case_is_either_in_scope_or_deferred() {
    let mut unmapped = Vec::new();
    let mut names = Vec::new();
    for case in unit_cases().into_iter().chain(adversarial_cases()) {
        let name = case.spec.name;
        names.push(name);
        let scoped = PHASE_CASES.contains(&name);
        let deferred = DEFERRED_CASES.iter().any(|(case, _)| *case == name);
        if scoped == deferred {
            unmapped.push(format!(
                "`{name}` is {}",
                if scoped {
                    "both in scope AND deferred"
                } else {
                    "in neither PHASE_CASES nor DEFERRED_CASES"
                }
            ));
        }
    }

    assert!(
        unmapped.is_empty(),
        "{} corpus case(s) with no clear engine-phase decision — add each to PHASE_CASES (the \
         engine draws its subject) or to DEFERRED_CASES with the phase it waits for:\n{}",
        unmapped.len(),
        unmapped.join("\n")
    );

    for name in PHASE_CASES
        .iter()
        .chain(DEFERRED_CASES.iter().map(|(c, _)| c))
    {
        assert!(
            names.contains(name),
            "`{name}` is named by this file but no longer exists in the corpus"
        );
    }
}

/// Every deferred case states a reason, and no reason is a bare placeholder.
#[test]
fn every_deferred_case_states_why_it_waits() {
    for (name, why) in DEFERRED_CASES {
        assert!(
            why.len() > 16,
            "`{name}`'s deferral reason is too thin to review: {why:?}"
        );
    }
}

/// Every escalation names a case that exists and is actually looser than the
/// tight default it replaces — a row that matched nothing, or that tightened
/// rather than widened, would be a silent no-op.
#[test]
fn every_escalation_is_a_real_widening_of_a_scoped_case() {
    let default = Tolerance::new();
    for (name, tolerance, why) in ESCALATIONS {
        assert!(
            PHASE_CASES.contains(name),
            "escalation for `{name}` names a case this phase does not compare"
        );
        assert!(
            tolerance.channel > default.channel
                || tolerance.alpha > default.alpha
                || tolerance.diff_pixels > default.diff_pixels,
            "escalation for `{name}` is not looser than the default {default:?}"
        );
        assert!(
            why.len() > 16,
            "`{name}`'s escalation reason is too thin to review: {why:?}"
        );
    }
}

/// The engine's unclassified fallback is never a committed golden directory —
/// the engine arm's own tripwire on the guard the other two arms carry.
#[test]
fn the_unclassified_engine_class_is_not_a_committed_golden_directory() {
    assert!(
        !goldens_root().join(ENGINE_UNCLASSIFIED_CLASS).exists(),
        "`{ENGINE_UNCLASSIFIED_CLASS}` is the refuse-to-promote fallback — it must never become \
         a committed golden class directory"
    );
}

/// The engine class this rig promotes into is the engine-prefixed twin of the
/// classic class captured on the same adapter, never the classic class itself.
#[test]
fn the_pinned_runners_engine_class_is_its_own_directory() {
    let class = engine_golden_class("vulkan", "NVIDIA T400 4GB");
    assert_eq!(class, "engine-vulkan-nvidia-t400");
    assert!(
        class.starts_with("engine-"),
        "an engine class is prefixed so it can never be read as a classic one"
    );
}

/// No scoped case shapes a glyph against a host font.
///
/// The gate above already refuses such a case as part of its own font check,
/// but it refuses it as one failure among many in a GPU run. This is the same
/// property stated on its own, on any machine — mirroring the identical test
/// in the other corpus gates.
#[test]
fn no_scoped_case_shapes_against_a_host_font() {
    let mut failures = Vec::new();
    for case in scoped_cases() {
        for message in foreign_font_runs(&case.scene()) {
            failures.push(format!("[{}] {message}", case.spec.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} non-portable frame(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
